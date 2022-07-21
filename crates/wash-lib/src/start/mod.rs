//! The `start` module contains functionality relating to downloading and starting
//! NATS servers and wasmCloud hosts.
//!
//! # Downloading and Starting NATS and wasmCloud
//! ```no_run
//! use anyhow::{anyhow, Result};
//! use wash_lib::start::{
//!     start_wasmcloud_host,
//!     start_nats_server,
//!     ensure_nats_server,
//!     ensure_wasmcloud
//! };
//! use std::path::PathBuf;
//!
//! #[tokio::main]
//! async fn main() -> Result<()> {
//!     let install_dir = PathBuf::from("/tmp");
//!     let os = std::env::consts::OS;
//!     let arch = std::env::consts::ARCH;
//!
//!     // Download NATS if not already installed
//!     let nats_binary = ensure_nats_server(os, arch, "v2.8.4", &install_dir).await?;
//!
//!     // Start NATS server, redirecting output to a log file
//!     let nats_log_path = install_dir.join("nats.log");
//!     let nats_log_file = std::fs::File::create(&nats_log_path)?;
//!     let mut nats_process = start_nats_server(
//!         nats_binary,
//!         nats_log_file,
//!         4222,
//!     )?;
//!     
//!     // Download wasmCloud if not already installed
//!     let wasmcloud_executable = ensure_wasmcloud(os, arch, "v0.55.1", &install_dir).await?;
//!     
//!     // Redirect output (which is on stderr) to a log file
//!     let log_path = install_dir.join("wasmcloud_stderr.log");
//!     let log_file = std::fs::File::create(&log_path)?;
//!     
//!     let mut wasmcloud_process = start_wasmcloud_host(
//!         wasmcloud_executable,
//!         std::process::Stdio::null(),
//!         log_file,
//!         std::collections::HashMap::new(),
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
mod nats;
pub use nats::*;
mod wasmcloud;
pub use wasmcloud::*;

#[cfg(test)]
pub(crate) mod test_helpers {
    /// Helper struct to ensure temp dirs are removed regardless of test result
    pub(crate) struct DirClean {
        pub(crate) dir: std::path::PathBuf,
    }
    impl Drop for DirClean {
        fn drop(&mut self) {
            println!("Removing temp dir {:?}", self.dir);
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    /// Helper struct to ensure spawned processes are killed regardless of test result
    pub(crate) struct ProcessChild {
        pub(crate) child: std::process::Child,
    }
    impl Drop for ProcessChild {
        fn drop(&mut self) {
            let _ = self.child.kill();
        }
    }
}
