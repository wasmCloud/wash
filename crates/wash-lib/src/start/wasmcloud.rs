use anyhow::{anyhow, Result};
use async_compression::tokio::bufread::GzipDecoder;
use std::io::Cursor;
#[cfg(target_family = "unix")]
use std::os::unix::prelude::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use tokio::fs::{create_dir_all, metadata, File};
use tokio_stream::StreamExt;
use tokio_tar::Archive;

const WASMCLOUD_GITHUB_RELEASE_URL: &str =
    "https://github.com/wasmCloud/wasmcloud-otp/releases/download";
#[cfg(target_family = "unix")]
pub(crate) const WASMCLOUD_HOST_BIN: &str = "bin/wasmcloud_host";
#[cfg(target_family = "windows")]
pub(crate) const WASMCLOUD_HOST_BIN: &str = "bin\\wasmcloud_host.bat";

/// Downloads the specified GitHub release version of the wasmCloud host from <https://github.com/wasmCloud/wasmcloud-otp/releases/>
/// and unpacks the contents for a specified OS/ARCH pair to a directory. Returns the path to the Elixir executable.
///
/// # Arguments
///
/// * `os` - Specifies the operating system of the binary to download, e.g. `linux`
/// * `arch` - Specifies the architecture of the binary to download, e.g. `amd64`
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `dir` - Where to unpack the wasmCloud host contents into
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use wash_lib::start::download_wasmcloud;
/// let os = std::env::consts::OS;
/// let arch = std::env::consts::ARCH;
/// let res = download_wasmcloud(os, arch, "v0.55.1", "/tmp/wasmcloud/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/wasmcloud/bin/wasmcloud_host".to_string());
/// # }
/// ```
pub async fn download_wasmcloud<P>(os: &str, arch: &str, version: &str, dir: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    if is_wasmcloud_installed(&dir).await {
        // wasmCloud already exists, return early
        return Ok(dir.as_ref().join(WASMCLOUD_HOST_BIN));
    }
    // Download wasmCloud host tarball
    let url = wasmcloud_url(os, arch, version);
    let body = reqwest::get(url).await?.bytes().await?;
    let cursor = Cursor::new(body);
    let mut wasmcloud_host = Archive::new(Box::new(GzipDecoder::new(cursor)));
    let mut entries = wasmcloud_host.entries()?;
    // Copy all of the files out of the tarball into the bin directory
    let mut executable_path = None;
    while let Some(res) = entries.next().await {
        let mut entry = res?;
        match entry.path() {
            Ok(path) => {
                let file_path = dir.as_ref().join(path);
                if let Some(parent_folder) = file_path.parent() {
                    create_dir_all(parent_folder).await?;
                }
                match File::create(&file_path).await {
                    Ok(mut wasmcloud_file) => {
                        // Set permissions of executable files and binaries to allow executing
                        if let Some(file_name) = file_path.file_name() {
                            let file_name = file_name.to_string_lossy();
                            #[cfg(target_family = "unix")]
                            if file_path.to_string_lossy().contains("bin")
                                || file_name.contains(".sh")
                                || file_name.contains(".bat")
                                || file_name.eq("wasmcloud_host")
                            {
                                let mut perms = wasmcloud_file.metadata().await?.permissions();
                                perms.set_mode(0o755);
                                wasmcloud_file.set_permissions(perms).await?;
                            }
                            // Set the executable path for return
                            if file_path.ends_with(WASMCLOUD_HOST_BIN) {
                                executable_path = Some(file_path.clone())
                            }
                        }
                        tokio::io::copy(&mut entry, &mut wasmcloud_file).await?;
                    }
                    Err(_e) => {
                        // This can occur both for folders (which always fail) and for permission denies, test
                        // valid failure scenarios and ensure we're only ignoring errors when it doesn't matter
                    }
                }
            }
            // Shouldn't happen, invalid path in tarball
            _ => (),
        }
    }

    // Return success if wasmCloud components exist, error otherwise
    match (is_wasmcloud_installed(&dir).await, executable_path) {
        (true, Some(path)) => Ok(path),
        (true, None) => Err(anyhow!(
            "wasmCloud was installed but the binary could not be located"
        )),
        (false, _) => Err(anyhow!(
            "wasmCloud was not installed successfully, please see logs"
        )),
    }
}

use std::collections::HashMap;
use std::ffi::OsStr;
/// Helper function to start a wasmCloud host given the path to the elixir release script
pub fn start_wasmcloud_host<P, T, S, K, V>(
    bin_path: P,
    stdout: T,
    stderr: S,
    env_vars: HashMap<K, V>,
) -> Result<Child>
where
    P: AsRef<Path>,
    T: Into<Stdio>,
    S: Into<Stdio>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    // wasmCloud host logs are sent to stderr as of https://github.com/wasmCloud/wasmcloud-otp/pull/418
    let mut cmd = &mut Command::new(bin_path.as_ref());

    // if let Some(port) = env_vars.get("PORT") {

    // }

    // Insert environment
    for (k, v) in env_vars {
        cmd = cmd.env(k, v)
    }

    // Spawn in the foreground so we can capture logs to a specified location
    cmd.stderr(stderr)
        .stdout(stdout)
        .arg("foreground")
        .spawn()
        .map_err(|e| anyhow!(e))
}

/// Helper function to ensure the wasmCloud host tarball is successfully
/// installed in a directory
pub async fn is_wasmcloud_installed<P>(dir: P) -> bool
where
    P: AsRef<Path>,
{
    use futures::future::join_all;
    let bin_dir = dir.as_ref().join("bin");
    let release_script = dir.as_ref().join(WASMCLOUD_HOST_BIN);
    let lib_dir = dir.as_ref().join("lib");
    let releases_dir = dir.as_ref().join("releases");
    let file_checks = vec![
        metadata(dir.as_ref()),
        metadata(&bin_dir),
        metadata(&release_script),
        metadata(&lib_dir),
        metadata(&releases_dir),
    ];
    join_all(file_checks).await.iter().all(|i| i.is_ok())
}

/// Helper function to determine the wasmCloud host release path given an os/arch and version
fn wasmcloud_url(os: &str, arch: &str, version: &str) -> String {
    format!(
        "{}/{}/{}-{}.tar.gz",
        WASMCLOUD_GITHUB_RELEASE_URL, version, arch, os
    )
}
#[cfg(test)]
mod test {
    use super::{download_wasmcloud, wasmcloud_url};
    use crate::start::{
        download_nats_server, is_nats_installed, is_wasmcloud_installed, start_nats_server,
        start_wasmcloud_host, NATS_SERVER_BINARY,
    };
    use reqwest::StatusCode;
    use std::{collections::HashMap, env::temp_dir};
    use tokio::fs::remove_dir_all;
    const WASMCLOUD_VERSION: &str = "v0.55.1";

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

    #[tokio::test]
    async fn can_request_supported_wasmcloud_urls() {
        let host_tarballs = vec![
            wasmcloud_url("linux", "aarch64", WASMCLOUD_VERSION),
            wasmcloud_url("linux", "x86_64", WASMCLOUD_VERSION),
            wasmcloud_url("macos", "aarch64", WASMCLOUD_VERSION),
            wasmcloud_url("macos", "x86_64", WASMCLOUD_VERSION),
            wasmcloud_url("windows", "x86_64", WASMCLOUD_VERSION),
        ];
        for tarball_url in host_tarballs {
            assert_eq!(
                reqwest::get(tarball_url).await.unwrap().status(),
                StatusCode::OK
            );
        }
    }

    #[tokio::test]
    async fn can_download_wasmcloud_tarball() {
        let download_dir = temp_dir().join("can_download_wasmcloud_tarball");
        let _cleanup_dir = DirClean {
            dir: download_dir.clone(),
        };
        let res: anyhow::Result<std::path::PathBuf> =
            download_wasmcloud("macos", "aarch64", WASMCLOUD_VERSION, &download_dir).await;
        assert!(res.is_ok());
        assert!(is_wasmcloud_installed(&download_dir).await);
        let _ = remove_dir_all(download_dir).await;
    }

    const NATS_SERVER_VERSION: &str = "v2.8.4";
    const WASMCLOUD_HOST_VERSION: &str = "v0.55.1";

    #[tokio::test]
    async fn can_gracefully_handle_multiple_hosts() -> anyhow::Result<()> {
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

        let mut host_env = HashMap::new();
        host_env.insert("WASMCLOUD_RPC_PORT", nats_port.to_string());
        host_env.insert("WASMCLOUD_CTL_PORT", nats_port.to_string());
        host_env.insert("WASMCLOUD_PROV_RPC_PORT", nats_port.to_string());
        let child_res = start_wasmcloud_host(
            &install_dir.join(crate::start::wasmcloud::WASMCLOUD_HOST_BIN),
            std::process::Stdio::null(),
            std::process::Stdio::null(),
            host_env,
        );
        assert!(child_res.is_ok());
        let _to_drop = ProcessChild {
            child: child_res.unwrap(),
        };

        Ok(())
    }
}
