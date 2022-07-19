use anyhow::Result;
use async_compression::tokio::bufread::GzipDecoder;
use std::path::Path;
use std::process::{Child, Command};
use std::{ffi::OsStr, io::Cursor, os::unix::prelude::PermissionsExt, path::PathBuf};
use tokio::fs::{create_dir_all, metadata, File};
use tokio_stream::StreamExt;
use tokio_tar::Archive;

const NATS_GITHUB_RELEASE_URL: &str = "https://github.com/nats-io/nats-server/releases/download";
const NATS_VERSION: &str = "v2.8.4";
const WASMCLOUD_GITHUB_RELEASE_URL: &str =
    "https://github.com/wasmCloud/wasmcloud-otp/releases/download";
const WASMCLOUD_VERSION: &str = "v0.55.1";

// Ideal, high level, usage from the wash cli
// handle start() {
// check nats parameters, see if connection can be established
//     take into account flags, contexts
// see if nats is downloaded to ~/.wash/bin
//     if not, download NATS, allow override with flag to disable this
//     start NATS, return connection info
// see if wasmCloud is downloaded to ~/.wash
//     if not, download wasmcloud, allow override with flag to disable this
//     start wasmCloud on 4000, atomic increment if the port is taken
// tell user to go to localhost:4000
// }

// considerations
// Take into account that the wasmCloud install will automatically use the host_config.json in ~/.wash as-is. Perhaps we should install to ~/.wash/bin/wasmcloud to compensate for this, users don't need to be able to run `wasmcloud_host` on their own and we could provide them with ~/.wash/bin/wasmcloud/bin as the thing to put in their path
// for the above, consider installing into ~/.wash/wasmcloud/bin

/// Downloads the specified GitHub release version of nats-server and unpacks the binary
/// for a specified OS/ARCH pair to a path from https://github.com/nats-io/nats-server/releases/
/// # Arguments
///
/// * `os` - Specifies the operating system of the binary to download, e.g. `linux`
/// * `arch` - Specifies the architecture of the binary to download, e.g. `amd64`
/// * `version` - Specifies the version of the binary to download in the form of `vX.Y.Z`
/// * `path` - Where to download the `nats-server` binary to
/// # Examples
///
/// ```
/// use wash_lib::start::download_nats_server;
/// let os = std::env::consts::OS;
/// let arch = std::env::consts::ARCH;
/// // Note: there's no need to `block_on` unless you're in a synchronous function
/// let res = tokio_test::block_on(
///     download_nats_server(os, arch, "v2.8.4", "/tmp/nats_server")
/// );
/// assert!(res.is_ok());
/// ```
pub async fn download_nats_server<P>(os: &str, arch: &str, version: &str, path: P) -> Result<()>
where
    P: AsRef<Path>,
{
    if let Ok(_md) = metadata(path.as_ref()).await {
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
                    if let Some(parent_folder) = path.as_ref().parent() {
                        create_dir_all(parent_folder).await?;
                    }
                    let mut nats_server = File::create(path.as_ref()).await?;
                    // Make nats-server executable
                    let mut permissions = nats_server.metadata().await?.permissions();
                    // Read/write/execute for owner and read/execute for others. This is what `cargo install` does
                    permissions.set_mode(0o755);
                    nats_server.set_permissions(permissions).await?;
                    tokio::io::copy(&mut entry, &mut nats_server).await?;
                    break;
                }
                // Ignore LICENSE and README in the NATS tarball
                _ => (),
            },
            // Shouldn't happen, invalid path in tarball, TODO: warn
            _ => (),
        }
    }

    //TODO: return error if no nats server found?
    Ok(())
}

/// Downloads the specified GitHub release version of nats-server and unpacks the binary
/// for a specified OS/ARCH pair to a path from https://github.com/wasmCloud/wasmcloud-otp/releases/
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
/// use wash_lib::start::download_wasmcloud;
/// let os = std::env::consts::OS;
/// let arch = std::env::consts::ARCH;
/// // Note: there's no need to `block_on` unless you're in a synchronous function
/// let res = tokio_test::block_on(
///     download_wasmcloud(os, arch, "v0.55.1", "/tmp/wasmcloud/")
/// );
/// assert!(res.is_ok());
/// ```
pub async fn download_wasmcloud<P>(os: &str, arch: &str, version: &str, dir: P) -> Result<()>
where
    P: AsRef<Path>,
{
    if let Ok(true) = ensure_wasmcloud_install(&dir).await {
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

                        tokio::io::copy(&mut entry, &mut wasmcloud_file).await?;
                    }
                    Err(_e) => {
                        // This can occur both for folders (which always fail) and for permission denies, test
                        // valid failure scenarios and ensure we're only ignoring errors when it doesn't matter
                        ()
                    }
                }
            }
            // Shouldn't happen, invalid path in tarball
            _ => (),
        }
    }

    //TODO: return error if no wasmcloud found?
    Ok(())
}

/// Helper function to determine the NATS server release path given an os/arch and version
fn nats_url(os: &str, arch: &str, version: &str) -> String {
    // Replace "macos" with "darwin" to match NATS release scheme
    let os = if os == "macos" { "darwin" } else { os };
    // Replace "aarch64" with "arm64" to match NATS release scheme
    let arch = if arch == "aarch64" { "arm64" } else { arch };
    format!(
        "{}/{}/nats-server-{}-{}-{}.tar.gz",
        NATS_GITHUB_RELEASE_URL, version, version, os, arch
    )
}

/// Helper function to determine the wasmCloud host release path given an os/arch and version
fn wasmcloud_url(os: &str, arch: &str, version: &str) -> String {
    format!(
        "{}/{}/{}-{}.tar.gz",
        WASMCLOUD_GITHUB_RELEASE_URL, version, arch, os
    )
}

// async fn run_nats() -> Result<Child> {
//     let cspec = Command::new(nats_path()).arg("-js").spawn()?;
//     println!("Child spec: {:?}", cspec);
//     Ok(cspec)
// }

// async fn run_wasmcloud() -> Result<Child> {
//     let cspec = Command::new(wasmcloud_path()).arg("start").spawn()?;
//     println!("Child spec: {:?}", cspec);
//     Ok(cspec)
// }

// fn nats_path() -> PathBuf {
//     //TODO: generalize with wash utils
//     PathBuf::from("/Users/brooks/.wash/bin/nats-server")
// }

// fn wasmcloud_path() -> PathBuf {
//     //TODO: generalize with wash utils
//     // we unpack so you can run `wasmcloud_host foreground`
//     PathBuf::from("/Users/brooks/.wash/bin/wasmcloud_host")
// }

// fn wash_path() -> PathBuf {
//     //TODO: generalize with wash utils
//     PathBuf::from("/Users/brooks/.wash")
// }

/// Helper function to ensure the wasmCloud host tarball is successfully
/// installed in a directory
async fn ensure_wasmcloud_install<P>(dir: P) -> Result<bool>
where
    P: AsRef<Path>,
{
    use futures::future::join_all;
    let bin_dir = dir.as_ref().join("bin");
    let lib_dir = dir.as_ref().join("lib");
    let releases_dir = dir.as_ref().join("releases");
    let file_checks = vec![
        metadata(dir.as_ref()),
        metadata(&bin_dir),
        metadata(&lib_dir),
        metadata(&releases_dir),
    ];
    Ok(join_all(file_checks)
        .await
        .iter()
        .fold(true, |acc, i| acc && i.is_ok()))
}

mod test {
    use super::{download_wasmcloud, ensure_wasmcloud_install, wasmcloud_url};
    use reqwest::StatusCode;
    use std::env::temp_dir;
    use tokio::fs::remove_dir_all;
    const WASMCLOUD_VERSION: &str = "v0.55.1";

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
        let res = download_wasmcloud("macos", "aarch64", WASMCLOUD_VERSION, &download_dir).await;
        assert!(res.is_ok());
        assert!(ensure_wasmcloud_install(&download_dir).await.unwrap());
        let _ = remove_dir_all(download_dir).await;
    }
}
