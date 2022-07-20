use anyhow::{anyhow, Result};
use async_compression::tokio::bufread::GzipDecoder;
#[cfg(target_family = "unix")]
use std::os::unix::prelude::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::{ffi::OsStr, io::Cursor};
use tokio::fs::{create_dir_all, metadata, File};
use tokio_stream::StreamExt;
use tokio_tar::Archive;

const NATS_GITHUB_RELEASE_URL: &str = "https://github.com/nats-io/nats-server/releases/download";
#[cfg(target_family = "unix")]
pub(crate) const NATS_SERVER_BINARY: &str = "nats-server";
#[cfg(target_family = "windows")]
pub(crate) const NATS_SERVER_BINARY: &str = "nats-server.exe";

/// Downloads the specified GitHub release version of nats-server from <https://github.com/nats-io/nats-server/releases/>
/// and unpacks the binary for a specified OS/ARCH pair to a directory. Returns the path to the NATS executable.
/// # Arguments
///
/// * `os` - Specifies the operating system of the binary to download, e.g. `linux`
/// * `arch` - Specifies the architecture of the binary to download, e.g. `amd64`
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `dir` - Where to download the `nats-server` binary to
/// # Examples
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// use wash_lib::start::download_nats_server;
/// let os = std::env::consts::OS;
/// let arch = std::env::consts::ARCH;
/// let res = download_nats_server(os, arch, "v2.8.4", "/tmp/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/nats-server");
/// # }
/// ```
pub async fn download_nats_server<P>(os: &str, arch: &str, version: &str, dir: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    let nats_bin_path = dir.as_ref().join(NATS_SERVER_BINARY);
    if let Ok(_md) = metadata(&nats_bin_path).await {
        // NATS already exists, return early
        return Ok(nats_bin_path);
    }
    // Download NATS tarball
    let url = nats_url(os, arch, version);
    let body = reqwest::get(url).await?.bytes().await?;
    let cursor = Cursor::new(body);
    let mut nats_server = Archive::new(Box::new(GzipDecoder::new(cursor)));

    // Look for nats-server binary and only extract that
    let mut entries = nats_server.entries()?;
    while let Some(res) = entries.next().await {
        let mut entry = res?;
        match entry.path() {
            Ok(tar_path) => match tar_path.file_name() {
                Some(name) if name == OsStr::new(NATS_SERVER_BINARY) => {
                    // Ensure target directory exists
                    create_dir_all(&dir).await?;
                    let mut nats_server = File::create(&nats_bin_path).await?;
                    // Make nats-server executable
                    #[cfg(target_family = "unix")]
                    {
                        let mut permissions = nats_server.metadata().await?.permissions();
                        // Read/write/execute for owner and read/execute for others. This is what `cargo install` does
                        permissions.set_mode(0o755);
                        nats_server.set_permissions(permissions).await?;
                    }

                    tokio::io::copy(&mut entry, &mut nats_server).await?;
                    return Ok(nats_bin_path);
                }
                // Ignore LICENSE and README in the NATS tarball
                _ => (),
            },
            // Shouldn't happen, invalid path in tarball
            _ => log::warn!("Invalid path detected in NATS release tarball, ignoring"),
        }
    }

    // Return success if NATS server binary exists, error otherwise
    Err(anyhow!(
        "NATS Server binary could not be installed, please see logs"
    ))
}

/// Helper function to execute a NATS server binary with wasmCloud arguments
/// # Arguments
///
/// * `bin_path` - Path to the nats-server binary to execute
/// * `stderr` - Specify where NATS stderr logs should be written to. If logs aren't important, use std::process::Stdio::null()
/// * `address` - Address for NATS to listen on
/// * `port` - Port for NATS to listen on
pub fn start_nats_server<P, T>(bin_path: P, stderr: T, address: &str, port: u16) -> Result<Child>
where
    P: AsRef<Path>,
    T: Into<Stdio>,
{
    if std::net::TcpListener::bind((address, port)).is_err() {
        return Err(anyhow!(
            "Could not start NATS server, a process is already listening on {}:{}",
            address,
            port
        ));
    }
    Command::new(bin_path.as_ref())
        .stderr(stderr)
        .arg("-js")
        .arg("--addr")
        .arg(address)
        .arg("--port")
        .arg(port.to_string())
        .spawn()
        .map_err(|e| anyhow!(e))
}

/// Helper function to ensure the NATS server binary is successfully
/// installed in a directory
pub async fn is_nats_installed<P>(dir: P) -> bool
where
    P: AsRef<Path>,
{
    metadata(dir.as_ref().join(NATS_SERVER_BINARY))
        .await
        .is_ok()
}

/// Helper function to determine the NATS server release path given an os/arch and version
fn nats_url(os: &str, arch: &str, version: &str) -> String {
    // Replace "macos" with "darwin" to match NATS release scheme
    let os = if os == "macos" { "darwin" } else { os };
    // Replace architecture to match NATS release naming scheme
    let arch = match arch {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        _ => arch,
    };
    format!(
        "{}/{}/nats-server-{}-{}-{}.tar.gz",
        NATS_GITHUB_RELEASE_URL, version, version, os, arch
    )
}

#[cfg(test)]
mod test {
    use crate::start::{
        download_nats_server, is_nats_installed, start_nats_server, NATS_SERVER_BINARY,
    };
    use anyhow::Result;
    use std::env::temp_dir;

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

    #[tokio::test]
    async fn can_gracefully_fail_running_nats() -> Result<()> {
        let install_dir = temp_dir().join("can_gracefully_fail_running_nats");
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

        let nats_one = start_nats_server(
            &install_dir.join(NATS_SERVER_BINARY),
            std::process::Stdio::null(),
            "0.0.0.0",
            10003,
        );
        assert!(nats_one.is_ok());
        let _to_drop = ProcessChild {
            child: nats_one.unwrap(),
        };

        // Give NATS a few seconds to start up and listen
        tokio::time::sleep(std::time::Duration::from_millis(5000)).await;
        let log_path = install_dir.join("nats.log");
        let log = std::fs::File::create(&log_path)?;
        let nats_two =
            start_nats_server(&install_dir.join(NATS_SERVER_BINARY), log, "0.0.0.0", 10003);
        assert!(nats_two.is_err());

        Ok(())
    }
}
