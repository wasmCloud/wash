use anyhow::{anyhow, Result};
use async_compression::tokio::bufread::GzipDecoder;
use std::io::Cursor;
#[cfg(target_family = "unix")]
use std::os::unix::prelude::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use tokio::fs::{create_dir_all, metadata, File};
use tokio_stream::StreamExt;
use tokio_tar::Archive;

const WASMCLOUD_GITHUB_RELEASE_URL: &str =
    "https://github.com/wasmCloud/wasmcloud-otp/releases/download";

/// Downloads the specified GitHub release version of nats-server and unpacks the binary
/// for a specified OS/ARCH pair to a path from <https://github.com/wasmCloud/wasmcloud-otp/releases/>
///
/// # Arguments
///
/// * `os` - Specifies the operating system of the binary to download, e.g. `linux`
/// * `arch` - Specifies the architecture of the binary to download, e.g. `amd64`
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `dir` - Where to unpack the wasmCloud host contents into
/// # Examples
///
/// ```
/// # #[tokio::main]
/// # async fn main() {
/// use wash_lib::start::download_wasmcloud;
/// let os = std::env::consts::OS;
/// let arch = std::env::consts::ARCH;
/// let res = download_wasmcloud(os, arch, "v0.55.1", "/tmp/wasmcloud/").await;
/// assert!(res.is_ok());
/// # }
/// ```
pub async fn download_wasmcloud<P>(os: &str, arch: &str, version: &str, dir: P) -> Result<()>
where
    P: AsRef<Path>,
{
    if is_wasmcloud_installed(&dir).await {
        // wasmCloud already exists, return early
        return Ok(());
    }
    // Download wasmCloud host tarball
    let url = wasmcloud_url(os, arch, version);
    let body = reqwest::get(url).await?.bytes().await?;
    let cursor = Cursor::new(body);
    let mut wasmcloud_host = Archive::new(Box::new(GzipDecoder::new(cursor)));
    let mut entries = wasmcloud_host.entries()?;
    // Copy all of the files out of the tarball into the bin directory
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
                        #[cfg(target_family = "unix")]
                        if let Some(file_name) = file_path.file_name() {
                            let file_name = file_name.to_string_lossy();
                            if file_name.contains(".sh")
                                // TODO: this will always be true if we install to ~/.wash/bin, should we set it always?
                                || file_path.to_string_lossy().contains("bin")
                                || file_name.contains(".bat")
                                || file_name.eq("wasmcloud_host")
                            {
                                let mut perms = wasmcloud_file.metadata().await?.permissions();
                                perms.set_mode(0o755);
                                wasmcloud_file.set_permissions(perms).await?;
                            }
                        }
                        //TODO: determine if the above is necessary on windows

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
    if is_wasmcloud_installed(&dir).await {
        Ok(())
    } else {
        Err(anyhow!(
            "wasmCloud was not installed successfully, please see logs"
        ))
    }
}

use std::collections::HashMap;
use std::ffi::OsStr;
/// Helper function to start a wasmCloud host given the path to the elixir release script
pub fn start_wasmcloud_host<P, T, K, V>(
    bin_path: P,
    log_file: T,
    env_vars: HashMap<K, V>,
) -> Result<Child>
where
    P: AsRef<Path>,
    T: Into<Stdio>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    // wasmCloud host logs are sent to stderr as of https://github.com/wasmCloud/wasmcloud-otp/pull/418
    let mut cmd = &mut Command::new(bin_path.as_ref());

    // Insert environment
    for (k, v) in env_vars {
        cmd = cmd.env(k, v)
    }

    // Spawn in the foreground so we can capture logs to a specified location
    cmd.stderr(log_file)
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
    //TODO: check for erts
    let bin_dir = dir.as_ref().join("bin");
    let release_script = dir.as_ref().join("bin/wasmcloud_host");
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
    use crate::start::is_wasmcloud_installed;
    use reqwest::StatusCode;
    use std::env::temp_dir;
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
        let download_dir = temp_dir();
        let _cleanup_dir = DirClean {
            dir: download_dir.clone(),
        };
        let res = download_wasmcloud("macos", "aarch64", WASMCLOUD_VERSION, &download_dir).await;
        assert!(res.is_ok());
        assert!(is_wasmcloud_installed(&download_dir).await);
        let _ = remove_dir_all(download_dir).await;
    }
}
