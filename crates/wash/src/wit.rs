//! WIT dependency management for wash components
//!
//! This module provides functionality to fetch and manage WebAssembly Interface Type (WIT)
//! dependencies for wasmCloud components. It integrates with the wasm-pkg-client for
//! fetching dependencies from registries and manages lock files for reproducible builds.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};
use wasm_pkg_client::{
    RegistryMapping,
    caching::{CachingClient, FileCache},
};
use wasm_pkg_core::{lock::LockFile, wit::OutputType};

/// The default name of the lock file for wasmCloud projects
pub const LOCK_FILE_NAME: &str = "wasmcloud.lock";
pub const WKG_LOCK_FILE_NAME: &str = "wkg.lock";

/// Configuration for WIT dependency management
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WitConfig {
    /// Registries for WIT package fetching (default: wasm.pkg registry)
    #[serde(default = "default_wit_registries")]
    pub registries: Vec<WitRegistry>,
    /// Skip fetching WIT dependencies
    #[serde(default)]
    pub skip_fetch: bool,
}

/// Default WIT registries (just the standard wasm.pkg registry)
fn default_wit_registries() -> Vec<WitRegistry> {
    // TODO(#1): bring BCA + wasmcloud here.
    vec![WitRegistry {
        url: "https://wasm.pkg".to_string(),
        token: None,
    }]
}

/// WIT registry configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WitRegistry {
    /// Registry URL
    pub url: String,

    /// Optional authentication token
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// Wrapper around a `wasm_pkg_client::Client` including configuration for fetching WIT dependencies.
/// Primarily enables reuse of functionality to override dependencies and properly setup the client.
pub struct WkgFetcher {
    wkg_config: wasm_pkg_core::config::Config,
    wkg_client_config: wasm_pkg_client::Config,
    cache: FileCache,
}

/// Common arguments for Wasm package tooling.
#[derive(Debug, Clone, Default)]
pub struct CommonPackageArgs {
    /// The path to the [wasm_pkg_client::Config] configuration file.
    pub config: Option<PathBuf>,
    /// The path to the cache directory. Defaults to the wash cache directory.
    pub cache: Option<PathBuf>,
}

impl CommonPackageArgs {
    /// Helper to load the config from the given path or other default paths
    pub async fn load_config(&self) -> anyhow::Result<wasm_pkg_client::Config> {
        // Get the default config so we have the default fallbacks
        let mut conf = wasm_pkg_client::Config::default();

        // We attempt to load config in the following order of preference:
        // 1. Path provided by the user via flag
        // 2. Path provided by the user via `WASH` prefixed environment variable
        // 3. Path provided by the users via `WKG` prefixed environment variable
        // 4. Default path to config file in wash dir
        // 5. Default path to config file from wkg
        match (self.config.as_ref(), std::env::var_os("WKG_CONFIG_FILE")) {
            // We have a config file provided by the user flag or WASH env var
            (Some(path), _) => {
                let loaded = wasm_pkg_client::Config::from_file(&path)
                    .await
                    .context(format!("error loading config file {path:?}"))?;
                // Merge the two configs
                conf.merge(loaded);
            }
            // We have a config file provided by the user via `WKG` env var
            (None, Some(path)) => {
                let loaded = wasm_pkg_client::Config::from_file(&path)
                    .await
                    .context(format!("error loading config file from {path:?}"))?;
                // Merge the two configs
                conf.merge(loaded);
            }
            // Otherwise we got nothing and attempt to load the default config locations
            (None, None) => {
                // TODO(#1): dot toml
                // let path = cfg_dir()?.join("package_config.toml");
                // let path = todo!("common dir");
                // Check if the config file exists before loading so we can error properly
                // if tokio::fs::metadata(&path).await.is_ok() {
                //     let loaded = wasm_pkg_client::Config::from_file(&path)
                //         .await
                //         .context(format!("error loading config file {path:?}"))?;
                //     // Merge the two configs
                //     conf.merge(loaded);
                // } else if let Ok(Some(c)) = wasm_pkg_client::Config::read_global_config().await {
                //     // This means the global config exists, so we merge that in instead
                //     conf.merge(c);
                // }
            }
        };
        let wasmcloud_label = "wasmcloud"
            .parse()
            .context("failed to parse wasmcloud label")?;
        // If they don't have a config set for the wasmcloud namespace, set it to the default defined here
        if conf.namespace_registry(&wasmcloud_label).is_none() {
            conf.set_namespace_registry(
                wasmcloud_label,
                RegistryMapping::Registry(
                    "wasmcloud.com"
                        .parse()
                        .context("failed to parse wasmcloud registry")?,
                ),
            );
        }
        // Same for wrpc
        let wrpc_label = "wrpc".parse().context("failed to parse wrpc label")?;
        if conf.namespace_registry(&wrpc_label).is_none() {
            conf.set_namespace_registry(
                wrpc_label,
                RegistryMapping::Registry(
                    "bytecodealliance.org"
                        .parse()
                        .context("failed to parse wrpc registry")?,
                ),
            );
        }
        Ok(conf)
    }

    /// Helper for loading the [`FileCache`]
    pub async fn load_cache(&self) -> anyhow::Result<FileCache> {
        // We attempt to setup a cache dir in the following order of preference:
        // 1. Path provided by the user via flag
        // 2. Path provided by the user via `WASH` prefixed environment variable
        // 3. Path provided by the users via `WKG` prefixed environment variable
        // 4. Default path to cache in wash dir
        let dir = match (self.cache.as_ref(), std::env::var_os("WKG_CACHE_DIR")) {
            // We have a cache dir provided by the user flag or WASH env var
            (Some(path), _) => path.to_owned(),
            // We have a cache dir provided by the user via `WKG` env var
            (None, Some(path)) => PathBuf::from(path),
            // Otherwise we got nothing and attempt to load the default cache dir
            // (None, None) => cfg_dir()?.join("package_cache"),
            _ => todo!("use common dir"),
        };
        FileCache::new(dir).await
    }
}

impl WkgFetcher {
    pub const fn new(
        wkg_config: wasm_pkg_core::config::Config,
        wkg_client_config: wasm_pkg_client::Config,
        cache: FileCache,
    ) -> Self {
        Self {
            wkg_config,
            wkg_client_config,
            cache,
        }
    }

    /// Load a `WkgFetcher` from a `CommonPackageArgs` and a `wasm_pkg_core::config::Config`
    pub async fn from_common(
        common: &CommonPackageArgs,
        wkg_config: wasm_pkg_core::config::Config,
    ) -> Result<Self> {
        let cache = common
            .load_cache()
            .await
            .context("failed to load wkg cache")?;
        let wkg_client_config = common
            .load_config()
            .await
            .context("failed to load wkg config")?;
        Ok(Self::new(wkg_config, wkg_client_config, cache))
    }

    // Enable extended pull configurations for wkg config. Call before calling `Self::get_client` to
    // update configuration used.
    // pub async fn resolve_extended_pull_configs(
    //     &mut self,
    //     pull_cfg: &RegistryPullConfig,
    //     wasmcloud_toml_dir: impl AsRef<Path>,
    // ) -> Result<()> {
    //     let wkg_config_overrides = self.wkg_config.overrides.get_or_insert_default();

    // for RegistryPullSourceOverride { target, source } in &pull_cfg.sources {
    //     let (ns, pkgs, _, _, maybe_version) = parse_wit_package_name(target)?;
    //     let version_suffix = maybe_version.map(|v| format!("@{v}")).unwrap_or_default();

    //     match source {
    //         // Local files can be used by adding them to the config
    //         RegistryPullSource::LocalPath(_) => {
    //             let path = source
    //                 .resolve_file_path(wasmcloud_toml_dir.as_ref())
    //                 .await?;
    //             if !tokio::fs::try_exists(&path).await.with_context(|| {
    //                 format!(
    //                     "failed to check for registry pull source local path [{}]",
    //                     path.display()
    //                 )
    //             })? {
    //                 bail!(
    //                     "registry pull source path [{}] does not exist",
    //                     path.display()
    //                 );
    //             }

    //             // Set the local override for the namespaces and/or packages
    //             update_override_dir(wkg_config_overrides, ns, pkgs.as_slice(), path);
    //         }
    //         RegistryPullSource::Builtin => bail!("no builtins are supported"),
    //         RegistryPullSource::RemoteHttp(s) => {
    //             let url = Url::parse(s)
    //                 .with_context(|| format!("invalid registry pull source url [{s}]"))?;
    //             let tempdir = tempfile::tempdir()
    //                 .with_context(|| {
    //                     format!("failed to create temp dir for downloading [{url}]")
    //                 })?
    //                 .keep();
    //             let output_path = tempdir.join("unpacked");
    //             let http_client = crate::lib::start::get_download_client()?;
    //             let req = http_client.get(url.clone()).send().await.with_context(|| {
    //                 format!("failed to retrieve WIT output from URL [{}]", &url)
    //             })?;
    //             let mut archive = tokio_tar::Archive::new(GzipDecoder::new(
    //                 tokio_util::io::StreamReader::new(req.bytes_stream().map_err(|e| {
    //                     std::io::Error::other(format!(
    //                     "failed to receive byte stream while downloading from URL [{}]: {e}",
    //                     &url
    //                 ))
    //                 })),
    //             ));
    //             archive.unpack(&output_path).await.with_context(|| {
    //                 format!("failed to unpack archive downloaded from URL [{}]", &url)
    //             })?;

    //             // Find the first nested directory named 'wit', if present
    //             let output_wit_dir = find_wit_folder_in_path(&output_path).await?;

    //             // Set the local override for the namespaces and/or packages
    //             // Set the local override for the namespaces and/or packages
    //             update_override_dir(wkg_config_overrides, ns, pkgs.as_slice(), output_wit_dir);
    //         }
    //         RegistryPullSource::RemoteGit(s) => {
    //             let url = Url::parse(s)
    //                 .with_context(|| format!("invalid registry pull source url [{s}]"))?;
    //             let query_pairs = url.query_pairs().collect::<HashMap<_, _>>();
    //             let tempdir = tempfile::tempdir()
    //                 .with_context(|| {
    //                     format!("failed to create temp dir for downloading [{url}]")
    //                 })?
    //                 .keep();

    //             // Determine the right git ref to use, based on the submitted query params
    //             let git_ref = match (
    //                 query_pairs.get("branch"),
    //                 query_pairs.get("sha"),
    //                 query_pairs.get("ref"),
    //             ) {
    //                 (Some(branch), _, _) => Some(RepoRef::Branch(String::from(branch.clone()))),
    //                 (_, Some(sha), _) => Some(RepoRef::from_str(sha)?),
    //                 (_, _, Some(r)) => Some(RepoRef::Unknown(String::from(r.clone()))),
    //                 _ => None,
    //             };

    //             clone_git_repo(
    //                 None,
    //                 &tempdir,
    //                 s.into(),
    //                 query_pairs
    //                     .get("subfolder")
    //                     .map(|s| String::from(s.clone())),
    //                 git_ref,
    //             )
    //             .await
    //             .with_context(|| {
    //                 format!("failed to clone repo for pull source git repo [{s}]",)
    //             })?;

    //             // Find the first nested directory named 'wit', if present
    //             let output_wit_dir = find_wit_folder_in_path(&tempdir).await?;

    //             // Set the local override for the namespaces and/or packages
    //             update_override_dir(wkg_config_overrides, ns, pkgs.as_slice(), output_wit_dir);
    //         }
    //         // All other registry pull sources should be directly convertible to `RegistryMapping`s
    //         rps @ (RegistryPullSource::RemoteOci(_)
    //         | RegistryPullSource::RemoteHttpWellKnown(_)) => {
    //             let registry = rps.clone().try_into()?;
    //             match (ns, pkgs.as_slice()) {
    //                 // namespace-level override
    //                 (ns, []) => {
    //                     self.wkg_client_config.set_namespace_registry(
    //                         format!("{ns}{version_suffix}").try_into()?,
    //                         registry,
    //                     );
    //                 }
    //                 // package-level override
    //                 (ns, packages) => {
    //                     for pkg in packages {
    //                         self.wkg_client_config.set_package_registry_override(
    //                             PackageRef::new(
    //                                 ns.clone().try_into()?,
    //                                 pkg.to_string().try_into()?,
    //                             ),
    //                             registry.clone(),
    //                         );
    //                     }
    //                 }
    //             }
    //         }
    //     }
    // }

    // Ok(())
    // }

    // Need to create WkgFetcher
    // then get lock file
    // then call this
    // then call resolve extended
    // then woo done
    // So, how create this
    // need these two things
    //   common: &CommonPackageArgs,
    // wkg_config: wasm_pkg_core::config::Config,
    pub async fn fetch_wit_dependencies(
        self,
        wit_dir: impl AsRef<Path>,
        lock: &mut LockFile,
    ) -> Result<()> {
        let client = CachingClient::new(
            Some(wasm_pkg_client::Client::new(self.wkg_client_config)),
            self.cache,
        );

        wasm_pkg_core::wit::fetch_dependencies(
            &self.wkg_config,
            wit_dir.as_ref(),
            lock,
            client,
            OutputType::Wit,
        )
        .await?;

        Ok(())
    }
}

/// Load a lock file from the project directory
///
/// The lock file is expected to be at `wkg.lock` in the project root, or `.wash/wasmcloud.lock` relative to the project directory.
#[instrument(skip_all)]
pub async fn load_lock_file(project_dir: impl AsRef<Path>) -> Result<LockFile> {
    let project_dir = project_dir.as_ref();

    let wkg_lock_file_path = project_dir.join(WKG_LOCK_FILE_NAME);
    let project_lock_file_path = project_dir.join(".wash").join(LOCK_FILE_NAME);

    let lock_file_path = if wkg_lock_file_path.exists() {
        // If the wkg.lock file exists, we prefer to use it
        debug!(path = %wkg_lock_file_path.display(), "found wkg.lock file, using it");
        Some(&wkg_lock_file_path)
    } else if project_lock_file_path.exists() {
        debug!(path = %project_lock_file_path.display(), "found .wash/wasmcloud.lock file, using it");
        Some(&project_lock_file_path)
    } else {
        None
    };

    if let Some(lock_file_path) = lock_file_path {
        debug!(lock_file_path = %lock_file_path.display(), "loading lock file ");
        // Check if the lock file is empty; if so, remove it and create a new lock file
        let metadata = tokio::fs::metadata(lock_file_path).await?;
        if metadata.len() == 0 {
            debug!("wkg.lock file is empty, removing and creating a new one");
            tokio::fs::remove_file(lock_file_path).await?;
            return LockFile::new_with_path([], lock_file_path)
                .await
                .context("failed to create new lock file after removing empty wkg.lock file");
        }
        LockFile::load_from_path(lock_file_path, false)
            .await
            .with_context(|| {
                format!(
                    "failed to load lock file: {lock_file_path}",
                    lock_file_path = lock_file_path.display()
                )
            })
    } else {
        debug!("lock file does not exist, will create when saving");
        // Ensure the .wash directory exists for when we create the file
        let wash_dir = project_dir.join(".wash");
        tokio::fs::create_dir_all(&wash_dir)
            .await
            .with_context(|| {
                format!(
                    "failed to create .wash directory: {wash_dir}",
                    wash_dir = wash_dir.display()
                )
            })?;

        // Create a new empty lock file that will be written to the path later
        LockFile::new_with_path([], &project_lock_file_path)
            .await
            .context("failed to create new lock file")
    }
}
