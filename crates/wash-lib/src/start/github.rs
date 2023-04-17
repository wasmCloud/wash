//! Reusable code for downloading tarballs from GitHub releases

use anyhow::{anyhow, Result};
use async_compression::tokio::bufread::GzipDecoder;
#[cfg(target_family = "unix")]
use std::os::unix::prelude::PermissionsExt;
use std::path::{Path, PathBuf};
use std::{ffi::OsStr, io::Cursor};
use tokio::fs::{create_dir_all, metadata, File};
use tokio_stream::StreamExt;
use tokio_tar::Archive;

/// Downloads the specified GitHub release version of nats-server from <https://github.com/nats-io/nats-server/releases/>
/// and unpacking the binary for a specified OS/ARCH pair to a directory. Returns the path to the NATS executable.
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
/// use wash_lib::start::download_nats_server_for_os_arch_pair;
/// let os = std::env::consts::OS;
/// let arch = std::env::consts::ARCH;
/// let res = download_nats_server_for_os_arch_pair(os, arch, "v2.8.4", "/tmp/").await;
/// assert!(res.is_ok());
/// assert!(res.unwrap().to_string_lossy() == "/tmp/nats-server");
/// # }
/// ```
pub async fn download_binary_from_github<P>(url: &str, dir: P, bin_name: &str) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    let bin_path = dir.as_ref().join(bin_name);
    // Download NATS tarball
    let body = match reqwest::get(url).await {
        Ok(resp) => resp.bytes().await?,
        Err(e) => return Err(anyhow!("Failed to request NATS release: {:?}", e)),
    };
    let cursor = Cursor::new(body);
    let mut bin_tarball = Archive::new(Box::new(GzipDecoder::new(cursor)));

    // Look for binary within tarball and only extract that
    let mut entries = bin_tarball.entries()?;
    while let Some(res) = entries.next().await {
        let mut entry = res.map_err(|e| {
            anyhow!(
                "Failed to retrieve file from archive, ensure {bin_name} exists. Original error: {e}",
            )
        })?;
        if let Ok(tar_path) = entry.path() {
            match tar_path.file_name() {
                Some(name) if name == OsStr::new(bin_name) => {
                    // Ensure target directory exists
                    create_dir_all(&dir).await?;
                    let mut bin_file = File::create(&bin_path).await?;
                    // Make binary executable
                    #[cfg(target_family = "unix")]
                    {
                        let mut permissions = bin_file.metadata().await?.permissions();
                        // Read/write/execute for owner and read/execute for others. This is what `cargo install` does
                        permissions.set_mode(0o755);
                        bin_file.set_permissions(permissions).await?;
                    }

                    tokio::io::copy(&mut entry, &mut bin_file).await?;
                    return Ok(bin_path);
                }
                // Ignore other files LICENSE and README in the tarball
                _ => (),
            }
        }
    }

    Err(anyhow!(
        "{bin_name} binary could not be installed, please see logs"
    ))
}

/// Helper function to determine if the provided binary is present in a directory
pub async fn is_bin_installed<P>(dir: P, bin_name: P) -> bool
where
    P: AsRef<Path>,
{
    metadata(dir.as_ref().join(bin_name))
        .await
        .map_or(false, |m| m.is_file())
}
