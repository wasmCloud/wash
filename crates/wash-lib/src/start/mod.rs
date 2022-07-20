//! The `start` module contains functionality relating to downloading and starting
//! NATS servers and wasmCloud hosts.
//!
//! # Downloading and Starting NATS and wasmCloud
//! ```rust
//! use anyhow::{anyhow, Result};
//! use wash_lib::start::{
//!     start_wasmcloud_host,
//!     start_nats_server,
//!     download_nats_server,
//!     download_wasmcloud,
//!     is_nats_installed,
//!     is_wasmcloud_installed
//! };
//! use std::{collections::HashMap, path::PathBuf};
//!
//! // Unix executables
//! #[cfg(target_family = "unix")]
//! const WASMCLOUD_HOST_BIN: &str = "bin/wasmcloud_host";
//! #[cfg(target_family = "unix")]
//! const NATS_BIN: &str = "nats-server";
//!
//! // Windows executables
//! #[cfg(target_family = "windows")]
//! const WASMCLOUD_HOST_BIN: &str = "bin\\wasmcloud_host.bat";
//! #[cfg(target_family = "windows")]
//! const NATS_BIN: &str = "nats-server.exe";
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let install_dir = PathBuf::from("/tmp");
//!     let os = std::env::consts::OS;
//!     let arch = std::env::consts::ARCH;
//!
//!     // Download NATS if not already installed
//!     if !is_nats_installed(&install_dir).await {
//!         download_nats_server(os, arch, "v2.8.4", &install_dir).await?;
//!     }
//!     // Start NATS server, redirecting output to a log file
//!     let nats_log_path = install_dir.join("nats.log");
//!     let nats_log_file = std::fs::File::create(&nats_log_path)?;
//!     let mut nats_process = start_nats_server(
//!         &install_dir.join(NATS_BIN),
//!         nats_log_file,
//!         "0.0.0.0",
//!         4222,
//!     )?;
//!     
//!     // Download wasmCloud if not already installed
//!     if !is_wasmcloud_installed(&install_dir).await {
//!         download_wasmcloud(os, arch, "v0.55.1", &install_dir).await?;
//!     }
//!     
//!     // Redirect output (which is on stderr) to a log file
//!     let log_path = install_dir.join("wasmcloud_stderr.log");
//!     let log_file = std::fs::File::create(&log_path)?;
//!     
//!     let env: HashMap<String, String> = HashMap::new();
//!     let mut wasmcloud_process = start_wasmcloud_host(
//!         &install_dir.join(WASMCLOUD_HOST_BIN),
//!         std::process::Stdio::null(),
//!         log_file,
//!         env,
//!     )?;
//!
//!     // Park thread, wasmCloud and NATS are running
//!     
//!     // Terminate processes
//!     nats_process.kill()?;
//!     wasmcloud_process.kill()?;
//!     Ok(())
//! }
//! ```
//TODO: audit exports
mod nats;
pub use nats::*;
mod wasmcloud;
pub use wasmcloud::*;

#[cfg(test)]
mod test {
    //TODO: is this how we should import them in cli?
    use crate::{
        start::nats::download_nats_server,
        start::nats::NATS_SERVER_BINARY,
        start::{download_wasmcloud, nats::start_nats_server, start_wasmcloud_host},
        start::{is_nats_installed, is_wasmcloud_installed},
    };
    use anyhow::Result;
    use std::{collections::HashMap, env::temp_dir};

    /// Helper struct to ensure temp dirs are removed regardless of test result
    struct DirClean {
        dir: std::path::PathBuf,
    }
    impl Drop for DirClean {
        fn drop(&mut self) {
            println!("Removing temp dir {:?}", self.dir);
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    /// Helper struct to ensure spawned processes are killed regardless of test result
    struct ProcessChild {
        child: std::process::Child,
    }
    impl Drop for ProcessChild {
        fn drop(&mut self) {
            let _ = self.child.kill();
        }
    }

    const NATS_SERVER_VERSION: &str = "v2.8.4";
    const WASMCLOUD_HOST_VERSION: &str = "v0.55.1";

    #[tokio::test]
    async fn can_download_and_start_nats() -> Result<()> {
        let install_dir = temp_dir().join("can_download_and_start_nats");
        let _cleanup_dir = DirClean {
            dir: install_dir.clone(),
        };
        assert!(!is_nats_installed(&install_dir).await);

        let res = download_nats_server(
            std::env::consts::OS,
            std::env::consts::ARCH,
            NATS_SERVER_VERSION,
            &install_dir,
        )
        .await;
        assert!(res.is_ok());

        let log_path = install_dir.join("nats.log");
        let log_file = tokio::fs::File::create(&log_path).await?.into_std().await;

        let child_res = start_nats_server(
            &install_dir.join(NATS_SERVER_BINARY),
            log_file,
            "0.0.0.0",
            10000,
        );
        assert!(child_res.is_ok());
        let _to_drop = ProcessChild {
            child: child_res.unwrap(),
        };

        // Give NATS max 5 seconds to start up
        for _ in 0..4 {
            let log_contents = tokio::fs::read_to_string(&log_path).await?;
            if log_contents.is_empty() {
                println!("NATS server hasn't started up yet, waiting 1 second");
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            } else {
                // Give just a little bit of time for the startup logs to flow in
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

                assert!(log_contents.contains("Starting nats-server"));
                assert!(log_contents.contains("Starting JetStream"));
                assert!(log_contents.contains("Server is ready"));
                break;
            }
        }

        Ok(())
    }

    #[tokio::test]
    async fn can_download_and_start_wasmcloud() -> Result<()> {
        let install_dir = temp_dir().join("can_download_and_start_wasmcloud");
        let _cleanup_dir = DirClean {
            dir: install_dir.clone(),
        };
        assert!(!is_wasmcloud_installed(&install_dir).await);
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;

        // Install and start NATS server for this test
        let nats_port = 10001;
        assert!(
            download_nats_server(os, arch, NATS_SERVER_VERSION, &install_dir)
                .await
                .is_ok()
        );
        assert!(is_nats_installed(&install_dir).await);
        let nats_child = start_nats_server(
            install_dir.join(NATS_SERVER_BINARY),
            std::process::Stdio::null(),
            "0.0.0.0",
            nats_port,
        );
        assert!(nats_child.is_ok());
        let _to_drop = ProcessChild {
            child: nats_child.unwrap(),
        };

        let res = download_wasmcloud(os, arch, WASMCLOUD_HOST_VERSION, &install_dir).await;
        assert!(res.is_ok());

        let stderr_log_path = install_dir.join("wasmcloud_stderr.log");
        let stderr_log_file = tokio::fs::File::create(&stderr_log_path)
            .await?
            .into_std()
            .await;
        let stdout_log_path = install_dir.join("wasmcloud_stdout.log");
        let stdout_log_file = tokio::fs::File::create(&stdout_log_path)
            .await?
            .into_std()
            .await;

        let mut host_env = HashMap::new();
        host_env.insert("WASMCLOUD_RPC_PORT", nats_port.to_string());
        host_env.insert("WASMCLOUD_CTL_PORT", nats_port.to_string());
        host_env.insert("WASMCLOUD_PROV_RPC_PORT", nats_port.to_string());
        let child_res = start_wasmcloud_host(
            &install_dir.join(crate::start::wasmcloud::WASMCLOUD_HOST_BIN),
            stdout_log_file,
            stderr_log_file,
            host_env,
        );
        assert!(child_res.is_ok());
        let _to_drop = ProcessChild {
            child: child_res.unwrap(),
        };

        // Give wasmCloud max 15 seconds to start up
        for _ in 0..14 {
            let log_contents = tokio::fs::read_to_string(&stderr_log_path).await?;
            if log_contents.is_empty() {
                println!("wasmCloud hasn't started up yet, waiting 1 second");
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            } else {
                // Give just a little bit of time for the startup logs to flow in, re-read logs
                tokio::time::sleep(std::time::Duration::from_millis(5000)).await;
                let log_contents = tokio::fs::read_to_string(&stderr_log_path).await?;
                assert!(log_contents
                    .contains("Connecting to control interface NATS without authentication"));
                assert!(
                    log_contents.contains("Connecting to lattice rpc NATS without authentication")
                );
                assert!(log_contents.contains("Started wasmCloud OTP Host Runtime"));
                break;
            }
        }

        Ok(())
    }
}
