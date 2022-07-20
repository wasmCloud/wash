use anyhow::{anyhow, Result};
use async_compression::tokio::bufread::GzipDecoder;
#[cfg(target_family = "unix")]
use std::os::unix::prelude::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::{ffi::OsStr, io::Cursor};
use tokio::fs::{create_dir_all, metadata, File};
use tokio_stream::StreamExt;
use tokio_tar::Archive;
const NATS_GITHUB_RELEASE_URL: &str = "https://github.com/nats-io/nats-server/releases/download";
pub(crate) const NATS_SERVER_BINARY: &str = "nats-server";

/// Downloads the specified GitHub release version of nats-server and unpacks the binary
/// for a specified OS/ARCH pair to a path from <https://github.com/nats-io/nats-server/releases/>
/// # Arguments
///
/// * `os` - Specifies the operating system of the binary to download, e.g. `linux`
/// * `arch` - Specifies the architecture of the binary to download, e.g. `amd64`
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `dir` - Where to download the `nats-server` binary to
/// # Examples
///
/// ```
/// # #[tokio::main]
/// # async fn main() {
/// use wash_lib::start::download_nats_server;
/// let os = std::env::consts::OS;
/// let arch = std::env::consts::ARCH;
/// let res = download_nats_server(os, arch, "v2.8.4", "/tmp/nats_server").await;
/// assert!(res.is_ok());
/// # }
/// ```
pub async fn download_nats_server<P>(os: &str, arch: &str, version: &str, dir: P) -> Result<()>
where
    P: AsRef<Path>,
{
    let nats_bin_path = dir.as_ref().join("nats-server");
    if let Ok(_md) = metadata(&nats_bin_path).await {
        // NATS already exists, return early
        return Ok(());
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
                Some(name) if name == OsStr::new("nats-server") => {
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
                    break;
                }
                // Ignore LICENSE and README in the NATS tarball
                _ => (),
            },
            // Shouldn't happen, invalid path in tarball
            _ => log::warn!("Invalid path detected in NATS release tarball, ignoring"),
        }
    }

    // Return success if NATS server binary exists, error otherwise
    if is_nats_installed(dir).await {
        Ok(())
    } else {
        Err(anyhow!(
            "NATS Server binary could not be installed, please see logs"
        ))
    }
}

/// Helper function to execute a NATS server binary with wasmCloud arguments
pub fn start_nats_for_wasmcloud<P, T>(bin_path: P, log_file: T) -> Result<Child>
where
    P: AsRef<Path>,
    T: Into<Stdio>,
{
    Command::new(bin_path.as_ref())
        .stderr(log_file)
        .arg("-js")
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
