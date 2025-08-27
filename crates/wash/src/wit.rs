//! WIT dependency management for wash components
//!
//! This module provides functionality to fetch and manage WebAssembly Interface Type (WIT)
//! dependencies for wasmCloud components. It integrates with the wasm-pkg-client for
//! fetching dependencies from registries and manages lock files for reproducible builds.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument};
use url::Url;
use wasm_pkg_client::{
    PackageRef, RegistryMapping,
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
    /// The directory where WIT files are stored, if not `./wit` in the project root
    #[serde(default)]
    pub wit_dir: Option<PathBuf>,
    /// Source overrides for WIT dependencies (target -> source mapping)
    #[serde(default)]
    pub sources: HashMap<String, String>,
    /// Disable WIT change detection optimization during development (default: false)
    #[serde(default)]
    pub disable_change_detection: bool,
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

/// Registry pull source types for WIT dependency overrides
#[derive(Debug, Clone)]
pub enum RegistryPullSource {
    /// Local filesystem path
    LocalPath(String),
    /// HTTP/HTTPS URL (tar.gz archives)
    RemoteHttp(String),
    /// Git repository URL
    RemoteGit(String),
    /// OCI registry reference
    RemoteOci(String),
}

impl TryFrom<RegistryPullSource> for RegistryMapping {
    type Error = anyhow::Error;

    fn try_from(source: RegistryPullSource) -> Result<Self, Self::Error> {
        match source {
            RegistryPullSource::RemoteOci(url) => Ok(RegistryMapping::Registry(url.parse()?)),
            _ => bail!("Cannot convert {:?} to RegistryMapping", source),
        }
    }
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
                // TODO(#1): support package_config.toml
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

    /// Enable extended pull configurations for wkg config. Call before calling `fetch_wit_dependencies` to
    /// update configuration used.
    pub async fn resolve_extended_pull_configs(
        &mut self,
        sources: &HashMap<String, String>,
        project_dir: impl AsRef<Path>,
    ) -> Result<()> {
        let wkg_config_overrides = self.wkg_config.overrides.get_or_insert_default();

        for (target, source) in sources {
            let (ns, pkgs, maybe_version) = parse_wit_package_name(target)?;
            let version_suffix = maybe_version.map(|v| format!("@{v}")).unwrap_or_default();

            let registry_pull_source = detect_source_type(source);

            match registry_pull_source {
                RegistryPullSource::LocalPath(_) => {
                    let resolved_path = project_dir.as_ref().join(source);
                    if !tokio::fs::try_exists(&resolved_path)
                        .await
                        .with_context(|| {
                            format!(
                                "failed to check for WIT source path [{}]",
                                resolved_path.display()
                            )
                        })?
                    {
                        bail!(
                            "WIT source path [{}] does not exist",
                            resolved_path.display()
                        );
                    }
                    set_override_for_target(wkg_config_overrides, &ns, &pkgs, resolved_path);
                }
                RegistryPullSource::RemoteHttp(_) => {
                    let wit_dir = download_and_extract_http(source)
                        .await
                        .with_context(|| format!("failed to download HTTP source [{}]", source))?;
                    set_override_for_target(wkg_config_overrides, &ns, &pkgs, wit_dir);
                }
                RegistryPullSource::RemoteGit(_) => {
                    let wit_dir = clone_git_and_find_wit(source)
                        .await
                        .with_context(|| format!("failed to clone Git source [{}]", source))?;
                    set_override_for_target(wkg_config_overrides, &ns, &pkgs, wit_dir);
                }
                RegistryPullSource::RemoteOci(_) => {
                    let registry = registry_pull_source.try_into()?;
                    match pkgs.as_slice() {
                        [] => {
                            // Namespace-level override
                            self.wkg_client_config.set_namespace_registry(
                                format!("{ns}{version_suffix}").try_into()?,
                                registry,
                            );
                        }
                        packages => {
                            // Package-level overrides
                            for pkg in packages {
                                self.wkg_client_config.set_package_registry_override(
                                    PackageRef::new(
                                        ns.clone().try_into()?,
                                        pkg.to_string().try_into()?,
                                    ),
                                    registry.clone(),
                                );
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

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

/// Detect source type from string format
fn detect_source_type(source: &str) -> RegistryPullSource {
    if source.starts_with("git+") || source.contains(".git") {
        RegistryPullSource::RemoteGit(source.to_string())
    } else if source.starts_with("http://") || source.starts_with("https://") {
        RegistryPullSource::RemoteHttp(source.to_string())
    } else if source.contains('/') && !source.starts_with('.') && !source.starts_with("file://") {
        // Likely OCI reference (contains slash but not relative path)
        RegistryPullSource::RemoteOci(source.to_string())
    } else {
        // Default to local path
        RegistryPullSource::LocalPath(source.to_string())
    }
}

/// Parse a WIT package name into namespace, packages, and version
/// Format: namespace:package@version or namespace@version
fn parse_wit_package_name(target: &str) -> Result<(String, Vec<String>, Option<String>)> {
    // Split on @ to separate version
    let (name_part, version) = if let Some((name, ver)) = target.rsplit_once('@') {
        (name, Some(ver.to_string()))
    } else {
        (target, None)
    };

    // Split on : to separate namespace and packages
    if let Some((namespace, packages_part)) = name_part.split_once(':') {
        let packages = if packages_part.is_empty() {
            vec![]
        } else {
            vec![packages_part.to_string()]
        };
        Ok((namespace.to_string(), packages, version))
    } else {
        // Just namespace, no packages
        Ok((name_part.to_string(), vec![], version))
    }
}

/// Set override for the target WIT package using the wkg config overrides map
fn set_override_for_target(
    overrides: &mut std::collections::HashMap<String, wasm_pkg_core::config::Override>,
    namespace: &str,
    packages: &[String],
    path: PathBuf,
) {
    use wasm_pkg_core::config::Override;

    if packages.is_empty() {
        // Namespace-level override: "namespace" = { path = "..." }
        overrides.insert(
            namespace.to_string(),
            Override {
                path: Some(path),
                version: None,
            },
        );
    } else {
        // Package-level override: "namespace:package" = { path = "..." }
        for package in packages {
            let key = format!("{}:{}", namespace, package);
            overrides.insert(
                key,
                Override {
                    path: Some(path.clone()),
                    version: None,
                },
            );
        }
    }
}

/// Download and extract HTTP tar.gz source
async fn download_and_extract_http(url: &str) -> Result<PathBuf> {
    let parsed_url = Url::parse(url).with_context(|| format!("invalid HTTP URL [{}]", url))?;

    let tempdir = tempfile::tempdir()
        .with_context(|| format!("failed to create temp dir for downloading [{}]", url))?
        .keep();

    let output_path = tempdir.join("unpacked");

    // Use reqwest to download the archive with rustls-tls
    let client = reqwest::ClientBuilder::new()
        .use_rustls_tls()
        .build()
        .context("failed to build HTTP client")?;
    let response = client
        .get(parsed_url)
        .send()
        .await
        .with_context(|| format!("failed to download from URL [{}]", url))?;

    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("failed to read response from URL [{}]", url))?;

    // Extract tar.gz
    let decoder = flate2::read::GzDecoder::new(&bytes[..]);
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(&output_path)
        .with_context(|| format!("failed to unpack archive from URL [{}]", url))?;

    find_wit_folder_in_path(&output_path).await
}

/// Clone git repository and find WIT directory
async fn clone_git_and_find_wit(url: &str) -> Result<PathBuf> {
    let tempdir = tempfile::tempdir()
        .with_context(|| format!("failed to create temp dir for cloning [{}]", url))?
        .keep();

    // Parse git URL to handle git+ prefix
    let git_url = if let Some(stripped) = url.strip_prefix("git+") {
        stripped
    } else {
        url
    };

    debug!(
        "cloning git repository: {} to {}",
        git_url,
        tempdir.display()
    );

    // Use system git command to clone the repository
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(["clone", git_url, tempdir.to_string_lossy().as_ref()]);

    let output = cmd
        .output()
        .await
        .with_context(|| format!("failed to execute git clone command for [{}]", git_url))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Git clone failed for [{}]: {}", git_url, stderr);
    }

    debug!(url = git_url, "successfully cloned git repository");

    let wit_path = find_wit_folder_in_path(&tempdir).await?;
    debug!(path = %wit_path.display(), "found WIT path");
    Ok(wit_path)
}

/// Find the WIT directory in the given path, preferring root-level 'wit' directories
async fn find_wit_folder_in_path(search_path: &Path) -> Result<PathBuf> {
    use tracing::debug;

    // First check if there's a 'wit' directory at the root level
    let root_wit = search_path.join("wit");
    if root_wit.exists() && root_wit.is_dir() {
        debug!(path = %root_wit.display(), "found root-level WIT directory");
        return Ok(root_wit);
    }

    // If no root-level wit directory, search recursively
    find_wit_folder_in_path_internal(search_path, 0).await
}

/// Internal helper for find_wit_folder_in_path with depth limit to prevent infinite recursion
fn find_wit_folder_in_path_internal(
    search_path: &Path,
    depth: usize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<PathBuf>> + Send + '_>> {
    Box::pin(async move {
        if depth > 10 {
            // Prevent infinite recursion
            return Ok(search_path.to_path_buf());
        }

        let mut entries = tokio::fs::read_dir(search_path)
            .await
            .with_context(|| format!("failed to read directory [{}]", search_path.display()))?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) == Some("wit") {
                    return Ok(path);
                }

                // Recursively search subdirectories
                if let Ok(nested_wit) = find_wit_folder_in_path_internal(&path, depth + 1).await
                    && nested_wit != path
                {
                    return Ok(nested_wit);
                }
            }
        }

        // If no wit directory found, return the search path itself
        Ok(search_path.to_path_buf())
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_detect_source_type() {
        assert!(matches!(
            detect_source_type("../shared-wit"),
            RegistryPullSource::LocalPath(_)
        ));
        assert!(matches!(
            detect_source_type("./local/path"),
            RegistryPullSource::LocalPath(_)
        ));
        assert!(matches!(
            detect_source_type("https://example.com/archive.tar.gz"),
            RegistryPullSource::RemoteHttp(_)
        ));
        assert!(matches!(
            detect_source_type("git+https://github.com/user/repo.git"),
            RegistryPullSource::RemoteGit(_)
        ));
        assert!(matches!(
            detect_source_type("ghcr.io/user/package"),
            RegistryPullSource::RemoteOci(_)
        ));
    }

    #[test]
    fn test_parse_wit_package_name() {
        let (ns, pkgs, ver) = parse_wit_package_name("wasmcloud:bus@1.0.0").unwrap();
        assert_eq!(ns, "wasmcloud");
        assert_eq!(pkgs, vec!["bus"]);
        assert_eq!(ver, Some("1.0.0".to_string()));

        let (ns, pkgs, ver) = parse_wit_package_name("wasi:config").unwrap();
        assert_eq!(ns, "wasi");
        assert_eq!(pkgs, vec!["config"]);
        assert_eq!(ver, None);

        let (ns, pkgs, ver) = parse_wit_package_name("wasmcloud@2.0.0").unwrap();
        assert_eq!(ns, "wasmcloud");
        assert_eq!(pkgs, Vec::<String>::new());
        assert_eq!(ver, Some("2.0.0".to_string()));
    }

    #[test]
    fn test_set_override_for_target() {
        let mut overrides = HashMap::new();
        let path = PathBuf::from("/tmp/test-wit");

        // Test namespace-level override
        set_override_for_target(&mut overrides, "wasmcloud", &[], path.clone());
        assert!(overrides.contains_key("wasmcloud"));
        assert_eq!(overrides["wasmcloud"].path, Some(path.clone()));

        // Test package-level override
        set_override_for_target(
            &mut overrides,
            "wasi",
            &["config".to_string()],
            path.clone(),
        );
        assert!(overrides.contains_key("wasi:config"));
        assert_eq!(overrides["wasi:config"].path, Some(path));
    }

    #[test]
    fn test_wit_config_deserialization() {
        let json = r#"
        {
            "sources": {
                "wasmcloud:bus": "../shared-wit",
                "wasi:config": "https://example.com/config.tar.gz"
            }
        }
        "#;

        let config: WitConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.sources.len(), 2);
        assert_eq!(config.sources["wasmcloud:bus"], "../shared-wit");
        assert_eq!(
            config.sources["wasi:config"],
            "https://example.com/config.tar.gz"
        );
    }
}

/// WIT change detection for optimizing development rebuilds
///
/// This module provides functionality to track changes to WIT files and their dependencies
/// to skip expensive WIT fetching operations during development when no relevant files have changed.
pub mod change_detection {
    use super::*;
    use anyhow::{Context, Result};
    use serde::{Deserialize, Serialize};
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
        time::SystemTime,
    };
    use tracing::{debug, trace};

    /// Cache for WIT file modification times and dependency metadata
    #[derive(Debug, Clone, Serialize, Deserialize, Default)]
    pub struct WitChangeCache {
        /// Last check timestamp
        pub last_check: Option<SystemTime>,
        /// Tracked WIT files with their modification times
        pub wit_files: HashMap<PathBuf, SystemTime>,
        /// Dependency configuration files with their modification times  
        pub dependency_files: HashMap<PathBuf, SystemTime>,
        /// Hash of the current WIT configuration
        pub wit_config_hash: Option<u64>,
    }

    impl WitChangeCache {
        /// Create a new empty WIT change cache
        pub fn new() -> Self {
            Self::default()
        }

        /// Load WIT change cache from the project's .wash directory
        pub async fn load(project_dir: &Path) -> Result<Self> {
            let cache_path = project_dir.join(".wash").join("wit-change-cache.json");

            if !cache_path.exists() {
                debug!("WIT change cache does not exist, creating new cache");
                return Ok(Self::new());
            }

            let cache_content =
                tokio::fs::read_to_string(&cache_path)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to read WIT change cache from {}",
                            cache_path.display()
                        )
                    })?;

            let cache: WitChangeCache = serde_json::from_str(&cache_content)
                .with_context(|| "failed to deserialize WIT change cache")?;

            debug!(
                "loaded WIT change cache with {} WIT files and {} dependency files",
                cache.wit_files.len(),
                cache.dependency_files.len()
            );
            Ok(cache)
        }

        /// Save WIT change cache to the project's .wash directory
        pub async fn save(&self, project_dir: &Path) -> Result<()> {
            let wash_dir = project_dir.join(".wash");
            tokio::fs::create_dir_all(&wash_dir)
                .await
                .with_context(|| {
                    format!("failed to create .wash directory at {}", wash_dir.display())
                })?;

            let cache_path = wash_dir.join("wit-change-cache.json");
            let cache_content = serde_json::to_string_pretty(self)
                .with_context(|| "failed to serialize WIT change cache")?;

            tokio::fs::write(&cache_path, cache_content)
                .await
                .with_context(|| {
                    format!(
                        "failed to write WIT change cache to {}",
                        cache_path.display()
                    )
                })?;

            debug!(
                "saved WIT change cache with {} WIT files and {} dependency files",
                self.wit_files.len(),
                self.dependency_files.len()
            );
            Ok(())
        }

        /// Update the cache with current file states
        pub async fn update_cache(
            &mut self,
            project_dir: &Path,
            wit_dir: &Path,
            wit_config: Option<&WitConfig>,
        ) -> Result<()> {
            self.last_check = Some(SystemTime::now());
            self.wit_files.clear();
            self.dependency_files.clear();

            // Update WIT config hash if provided
            if let Some(config) = wit_config {
                self.wit_config_hash = Some(calculate_wit_config_hash(config));
            }

            // Track WIT files
            if wit_dir.exists() {
                self.track_wit_files(wit_dir).await?;
            }

            // Track dependency configuration files
            self.track_dependency_files(project_dir).await?;

            Ok(())
        }

        /// Track all WIT files in the given directory recursively
        async fn track_wit_files(&mut self, wit_dir: &Path) -> Result<()> {
            let mut stack = vec![wit_dir.to_path_buf()];

            while let Some(current_dir) = stack.pop() {
                let mut entries = tokio::fs::read_dir(&current_dir).await.with_context(|| {
                    format!("failed to read WIT directory: {}", current_dir.display())
                })?;

                while let Some(entry) = entries.next_entry().await? {
                    let path = entry.path();
                    let metadata = entry.metadata().await?;

                    if metadata.is_dir() {
                        // Skip hidden directories and common build output directories
                        if let Some(name) = path.file_name().and_then(|n| n.to_str())
                            && !name.starts_with('.') && !name.starts_with("_") {
                            stack.push(path);
                        }
                    } else if metadata.is_file()
                        && let Some(ext) = path.extension().and_then(|e| e.to_str())
                        && ext == "wit" {
                        let modified = metadata.modified().with_context(|| {
                            format!(
                                "failed to get modification time for {}",
                                path.display()
                            )
                        })?;
                        self.wit_files.insert(path.clone(), modified);
                        trace!("tracked WIT file: {}", path.display());
                    }
                }
            }

            debug!("tracked {} WIT files", self.wit_files.len());
            Ok(())
        }

        /// Track dependency configuration files
        async fn track_dependency_files(&mut self, project_dir: &Path) -> Result<()> {
            let dependency_files = [
                "wit.toml",
                "Cargo.toml",
                "package.json",
                "go.mod",
                ".wash/config.json",
                "wkg.lock",
                ".wash/wasmcloud.lock",
            ];

            for file_name in &dependency_files {
                let file_path = project_dir.join(file_name);
                if file_path.exists() {
                    let metadata = tokio::fs::metadata(&file_path).await.with_context(|| {
                        format!("failed to get metadata for {}", file_path.display())
                    })?;
                    let modified = metadata.modified().with_context(|| {
                        format!(
                            "failed to get modification time for {}",
                            file_path.display()
                        )
                    })?;
                    self.dependency_files.insert(file_path.clone(), modified);
                    trace!("tracked dependency file: {}", file_path.display());
                }
            }

            debug!("tracked {} dependency files", self.dependency_files.len());
            Ok(())
        }
    }

    /// Calculate a hash of the WIT configuration for change detection
    fn calculate_wit_config_hash(wit_config: &WitConfig) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();

        // Hash the registries
        for registry in &wit_config.registries {
            registry.url.hash(&mut hasher);
            // Note: We don't hash the token for security reasons
        }

        // Hash skip_fetch setting
        wit_config.skip_fetch.hash(&mut hasher);

        // Hash wit_dir if present
        if let Some(wit_dir) = &wit_config.wit_dir {
            wit_dir.hash(&mut hasher);
        }

        // Hash source overrides
        for (key, value) in &wit_config.sources {
            key.hash(&mut hasher);
            value.hash(&mut hasher);
        }

        hasher.finish()
    }

    /// Check if WIT files or dependencies have changed since the last check
    pub async fn has_wit_files_changed(
        project_dir: &Path,
        wit_dir: &Path,
        wit_config: Option<&WitConfig>,
        cache: &WitChangeCache,
    ) -> Result<bool> {
        // If we have no previous cache data, assume files have changed
        if cache.last_check.is_none() {
            debug!("no previous WIT change cache found, assuming files have changed");
            return Ok(true);
        }

        // Check if WIT configuration has changed
        if let Some(config) = wit_config {
            let current_hash = calculate_wit_config_hash(config);
            if cache.wit_config_hash != Some(current_hash) {
                debug!("WIT configuration has changed, forcing WIT fetch");
                return Ok(true);
            }
        }

        // Check WIT files for changes
        if wit_dir.exists() {
            if check_wit_files_changed(wit_dir, &cache.wit_files).await? {
                return Ok(true);
            }
        } else if !cache.wit_files.is_empty() {
            // WIT directory was removed
            debug!("WIT directory no longer exists but cache had WIT files, assuming changed");
            return Ok(true);
        }

        // Check dependency files for changes
        if check_dependency_files_changed(project_dir, &cache.dependency_files).await? {
            return Ok(true);
        }

        debug!("no WIT file changes detected since last check");
        Ok(false)
    }

    /// Check if WIT files have been modified
    async fn check_wit_files_changed(
        wit_dir: &Path,
        cached_files: &HashMap<PathBuf, SystemTime>,
    ) -> Result<bool> {
        let mut current_files = HashMap::new();
        let mut stack = vec![wit_dir.to_path_buf()];

        // Collect all current WIT files
        while let Some(current_dir) = stack.pop() {
            let mut entries = tokio::fs::read_dir(&current_dir).await.with_context(|| {
                format!("failed to read WIT directory: {}", current_dir.display())
            })?;

            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                let metadata = entry.metadata().await?;

                if metadata.is_dir() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str())
                        && !name.starts_with('.') && !name.starts_with("_") {
                        stack.push(path);
                    }
                } else if metadata.is_file()
                    && let Some(ext) = path.extension().and_then(|e| e.to_str())
                    && ext == "wit" {
                    let modified = metadata.modified().with_context(|| {
                        format!("failed to get modification time for {}", path.display())
                    })?;
                    current_files.insert(path.clone(), modified);
                }
            }
        }

        // Check if the file sets are different
        if current_files.len() != cached_files.len() {
            debug!(
                "number of WIT files changed: {} -> {}",
                cached_files.len(),
                current_files.len()
            );
            return Ok(true);
        }

        // Check each file for modifications
        for (file_path, current_modified) in &current_files {
            match cached_files.get(file_path) {
                Some(cached_modified) => {
                    if current_modified != cached_modified {
                        debug!("WIT file modified: {}", file_path.display());
                        return Ok(true);
                    }
                }
                None => {
                    debug!("new WIT file found: {}", file_path.display());
                    return Ok(true);
                }
            }
        }

        // Check for deleted files
        for file_path in cached_files.keys() {
            if !current_files.contains_key(file_path) {
                debug!("WIT file deleted: {}", file_path.display());
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Check if dependency configuration files have been modified
    async fn check_dependency_files_changed(
        project_dir: &Path,
        cached_files: &HashMap<PathBuf, SystemTime>,
    ) -> Result<bool> {
        let dependency_files = [
            "wit.toml",
            "Cargo.toml",
            "package.json",
            "go.mod",
            ".wash/config.json",
            "wkg.lock",
            ".wash/wasmcloud.lock",
        ];

        let mut current_files = HashMap::new();

        // Check each possible dependency file
        for file_name in &dependency_files {
            let file_path = project_dir.join(file_name);
            if file_path.exists() {
                let metadata = tokio::fs::metadata(&file_path).await.with_context(|| {
                    format!("failed to get metadata for {}", file_path.display())
                })?;
                let modified = metadata.modified().with_context(|| {
                    format!(
                        "failed to get modification time for {}",
                        file_path.display()
                    )
                })?;
                current_files.insert(file_path, modified);
            }
        }

        // Check if the file sets are different
        if current_files.len() != cached_files.len() {
            debug!(
                "number of dependency files changed: {} -> {}",
                cached_files.len(),
                current_files.len()
            );
            return Ok(true);
        }

        // Check each file for modifications
        for (file_path, current_modified) in &current_files {
            match cached_files.get(file_path) {
                Some(cached_modified) => {
                    if current_modified != cached_modified {
                        debug!("dependency file modified: {}", file_path.display());
                        return Ok(true);
                    }
                }
                None => {
                    debug!("new dependency file found: {}", file_path.display());
                    return Ok(true);
                }
            }
        }

        // Check for deleted files
        for file_path in cached_files.keys() {
            if !current_files.contains_key(file_path) {
                debug!("dependency file deleted: {}", file_path.display());
                return Ok(true);
            }
        }

        Ok(false)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::{fs, time::SystemTime};
        use tempfile::TempDir;

        #[tokio::test]
        async fn test_wit_change_cache_new() {
            let cache = WitChangeCache::new();
            assert!(cache.last_check.is_none());
            assert!(cache.wit_files.is_empty());
            assert!(cache.dependency_files.is_empty());
            assert!(cache.wit_config_hash.is_none());
        }

        #[tokio::test]
        async fn test_wit_change_cache_load_nonexistent() {
            let temp_dir = TempDir::new().expect("failed to create temp dir");
            let project_dir = temp_dir.path();

            let cache = WitChangeCache::load(project_dir)
                .await
                .expect("failed to load cache");
            assert!(cache.last_check.is_none());
            assert!(cache.wit_files.is_empty());
            assert!(cache.dependency_files.is_empty());
        }

        #[tokio::test]
        async fn test_wit_change_cache_save_and_load() {
            let temp_dir = TempDir::new().expect("failed to create temp dir");
            let project_dir = temp_dir.path();

            let mut cache = WitChangeCache::new();
            cache.last_check = Some(SystemTime::now());
            cache
                .wit_files
                .insert(PathBuf::from("wit/world.wit"), SystemTime::now());
            cache
                .dependency_files
                .insert(PathBuf::from("Cargo.toml"), SystemTime::now());
            cache.wit_config_hash = Some(12345);

            // Save the cache
            cache.save(project_dir).await.expect("failed to save cache");

            // Load the cache
            let loaded_cache = WitChangeCache::load(project_dir)
                .await
                .expect("failed to load cache");
            assert!(loaded_cache.last_check.is_some());
            assert_eq!(loaded_cache.wit_files.len(), 1);
            assert_eq!(loaded_cache.dependency_files.len(), 1);
            assert_eq!(loaded_cache.wit_config_hash, Some(12345));
            assert!(
                loaded_cache
                    .wit_files
                    .contains_key(&PathBuf::from("wit/world.wit"))
            );
            assert!(
                loaded_cache
                    .dependency_files
                    .contains_key(&PathBuf::from("Cargo.toml"))
            );
        }

        #[tokio::test]
        async fn test_wit_change_cache_update_cache() {
            let temp_dir = TempDir::new().expect("failed to create temp dir");
            let project_dir = temp_dir.path();
            let wit_dir = project_dir.join("wit");

            // Create WIT directory and files
            fs::create_dir_all(&wit_dir).expect("failed to create wit dir");
            let wit_file1 = wit_dir.join("world.wit");
            let wit_file2 = wit_dir.join("types.wit");
            fs::write(&wit_file1, "package example:world;").expect("failed to write wit file");
            fs::write(&wit_file2, "interface types {}").expect("failed to write wit file");

            // Create dependency files
            let cargo_toml = project_dir.join("Cargo.toml");
            fs::write(&cargo_toml, "[package]\nname = \"test\"")
                .expect("failed to write cargo.toml");

            let mut cache = WitChangeCache::new();
            let wit_config = WitConfig::default();

            // Update cache
            cache
                .update_cache(project_dir, &wit_dir, Some(&wit_config))
                .await
                .expect("failed to update cache");

            assert!(cache.last_check.is_some());
            assert_eq!(cache.wit_files.len(), 2);
            assert_eq!(cache.dependency_files.len(), 1);
            assert!(cache.wit_files.contains_key(&wit_file1));
            assert!(cache.wit_files.contains_key(&wit_file2));
            assert!(cache.dependency_files.contains_key(&cargo_toml));
            assert!(cache.wit_config_hash.is_some());
        }

        #[tokio::test]
        async fn test_calculate_wit_config_hash() {
            let config1 = WitConfig {
                skip_fetch: false,
                wit_dir: Some(PathBuf::from("wit")),
                sources: [("test:package".to_string(), "../local".to_string())].into(),
                ..Default::default()
            };

            let config2 = WitConfig {
                skip_fetch: true, // Different skip_fetch
                wit_dir: Some(PathBuf::from("wit")),
                sources: [("test:package".to_string(), "../local".to_string())].into(),
                ..Default::default()
            };

            let config3 = WitConfig {
                skip_fetch: false,
                wit_dir: Some(PathBuf::from("different")), // Different wit_dir
                sources: [("test:package".to_string(), "../local".to_string())].into(),
                ..Default::default()
            };

            let hash1 = calculate_wit_config_hash(&config1);
            let hash2 = calculate_wit_config_hash(&config2);
            let hash3 = calculate_wit_config_hash(&config3);

            assert_ne!(hash1, hash2);
            assert_ne!(hash1, hash3);
            assert_ne!(hash2, hash3);

            // Same config should produce same hash
            let hash1_duplicate = calculate_wit_config_hash(&config1);
            assert_eq!(hash1, hash1_duplicate);
        }

        #[tokio::test]
        async fn test_has_wit_files_changed_no_cache() {
            let temp_dir = TempDir::new().expect("failed to create temp dir");
            let project_dir = temp_dir.path();
            let wit_dir = project_dir.join("wit");

            let empty_cache = WitChangeCache::new();

            // Should return true when no cache data exists
            let result = has_wit_files_changed(project_dir, &wit_dir, None, &empty_cache)
                .await
                .expect("failed to check wit files changed");
            assert!(result);
        }

        #[tokio::test]
        async fn test_has_wit_files_changed_config_change() {
            let temp_dir = TempDir::new().expect("failed to create temp dir");
            let project_dir = temp_dir.path();
            let wit_dir = project_dir.join("wit");

            let mut cache = WitChangeCache::new();
            cache.last_check = Some(SystemTime::now());

            let config1 = WitConfig {
                skip_fetch: false,
                ..Default::default()
            };
            let config2 = WitConfig {
                skip_fetch: true, // Different config
                ..Default::default()
            };

            // Set cache with config1 hash
            cache.wit_config_hash = Some(calculate_wit_config_hash(&config1));

            // Should return true when config changes
            let result = has_wit_files_changed(project_dir, &wit_dir, Some(&config2), &cache)
                .await
                .expect("failed to check wit files changed");
            assert!(result);
        }

        #[tokio::test]
        async fn test_has_wit_files_changed_no_changes() {
            let temp_dir = TempDir::new().expect("failed to create temp dir");
            let project_dir = temp_dir.path();
            let wit_dir = project_dir.join("wit");

            // Create WIT directory and file
            fs::create_dir_all(&wit_dir).expect("failed to create wit dir");
            let wit_file = wit_dir.join("world.wit");
            fs::write(&wit_file, "package example:world;").expect("failed to write wit file");

            let mut cache = WitChangeCache::new();
            let wit_config = WitConfig::default();

            // Update cache with current state
            cache
                .update_cache(project_dir, &wit_dir, Some(&wit_config))
                .await
                .expect("failed to update cache");

            // Should return false when nothing has changed
            let result = has_wit_files_changed(project_dir, &wit_dir, Some(&wit_config), &cache)
                .await
                .expect("failed to check wit files changed");
            assert!(!result);
        }

        #[tokio::test]
        async fn test_has_wit_files_changed_wit_file_modified() {
            let temp_dir = TempDir::new().expect("failed to create temp dir");
            let project_dir = temp_dir.path();
            let wit_dir = project_dir.join("wit");

            // Create WIT directory and file
            fs::create_dir_all(&wit_dir).expect("failed to create wit dir");
            let wit_file = wit_dir.join("world.wit");
            fs::write(&wit_file, "package example:world;").expect("failed to write wit file");

            let mut cache = WitChangeCache::new();
            let wit_config = WitConfig::default();

            // Update cache with current state
            cache
                .update_cache(project_dir, &wit_dir, Some(&wit_config))
                .await
                .expect("failed to update cache");

            // Modify the WIT file
            tokio::time::sleep(std::time::Duration::from_millis(10)).await; // Ensure different timestamp
            fs::write(&wit_file, "package example:world;\n// modified")
                .expect("failed to modify wit file");

            // Should return true when WIT file is modified
            let result = has_wit_files_changed(project_dir, &wit_dir, Some(&wit_config), &cache)
                .await
                .expect("failed to check wit files changed");
            assert!(result);
        }

        #[tokio::test]
        async fn test_has_wit_files_changed_dependency_file_modified() {
            let temp_dir = TempDir::new().expect("failed to create temp dir");
            let project_dir = temp_dir.path();
            let wit_dir = project_dir.join("wit");

            // Create dependency file
            let cargo_toml = project_dir.join("Cargo.toml");
            fs::write(&cargo_toml, "[package]\nname = \"test\"")
                .expect("failed to write cargo.toml");

            let mut cache = WitChangeCache::new();
            let wit_config = WitConfig::default();

            // Update cache with current state
            cache
                .update_cache(project_dir, &wit_dir, Some(&wit_config))
                .await
                .expect("failed to update cache");

            // Modify the dependency file
            tokio::time::sleep(std::time::Duration::from_millis(10)).await; // Ensure different timestamp
            fs::write(
                &cargo_toml,
                "[package]\nname = \"test\"\nversion = \"0.1.0\"",
            )
            .expect("failed to modify cargo.toml");

            // Should return true when dependency file is modified
            let result = has_wit_files_changed(project_dir, &wit_dir, Some(&wit_config), &cache)
                .await
                .expect("failed to check wit files changed");
            assert!(result);
        }

        #[tokio::test]
        async fn test_has_wit_files_changed_new_wit_file() {
            let temp_dir = TempDir::new().expect("failed to create temp dir");
            let project_dir = temp_dir.path();
            let wit_dir = project_dir.join("wit");

            // Create WIT directory and file
            fs::create_dir_all(&wit_dir).expect("failed to create wit dir");
            let wit_file1 = wit_dir.join("world.wit");
            fs::write(&wit_file1, "package example:world;").expect("failed to write wit file");

            let mut cache = WitChangeCache::new();
            let wit_config = WitConfig::default();

            // Update cache with current state
            cache
                .update_cache(project_dir, &wit_dir, Some(&wit_config))
                .await
                .expect("failed to update cache");

            // Add a new WIT file
            let wit_file2 = wit_dir.join("types.wit");
            fs::write(&wit_file2, "interface types {}").expect("failed to write new wit file");

            // Should return true when new WIT file is added
            let result = has_wit_files_changed(project_dir, &wit_dir, Some(&wit_config), &cache)
                .await
                .expect("failed to check wit files changed");
            assert!(result);
        }

        #[tokio::test]
        async fn test_has_wit_files_changed_wit_file_deleted() {
            let temp_dir = TempDir::new().expect("failed to create temp dir");
            let project_dir = temp_dir.path();
            let wit_dir = project_dir.join("wit");

            // Create WIT directory and files
            fs::create_dir_all(&wit_dir).expect("failed to create wit dir");
            let wit_file1 = wit_dir.join("world.wit");
            let wit_file2 = wit_dir.join("types.wit");
            fs::write(&wit_file1, "package example:world;").expect("failed to write wit file");
            fs::write(&wit_file2, "interface types {}").expect("failed to write wit file");

            let mut cache = WitChangeCache::new();
            let wit_config = WitConfig::default();

            // Update cache with current state
            cache
                .update_cache(project_dir, &wit_dir, Some(&wit_config))
                .await
                .expect("failed to update cache");

            // Delete a WIT file
            fs::remove_file(&wit_file2).expect("failed to delete wit file");

            // Should return true when WIT file is deleted
            let result = has_wit_files_changed(project_dir, &wit_dir, Some(&wit_config), &cache)
                .await
                .expect("failed to check wit files changed");
            assert!(result);
        }

        #[tokio::test]
        async fn test_has_wit_files_changed_wit_directory_removed() {
            let temp_dir = TempDir::new().expect("failed to create temp dir");
            let project_dir = temp_dir.path();
            let wit_dir = project_dir.join("wit");

            // Create WIT directory and file
            fs::create_dir_all(&wit_dir).expect("failed to create wit dir");
            let wit_file = wit_dir.join("world.wit");
            fs::write(&wit_file, "package example:world;").expect("failed to write wit file");

            let mut cache = WitChangeCache::new();
            let wit_config = WitConfig::default();

            // Update cache with current state
            cache
                .update_cache(project_dir, &wit_dir, Some(&wit_config))
                .await
                .expect("failed to update cache");

            // Remove entire WIT directory
            fs::remove_dir_all(&wit_dir).expect("failed to remove wit directory");

            // Should return true when WIT directory is removed but cache had WIT files
            let result = has_wit_files_changed(project_dir, &wit_dir, Some(&wit_config), &cache)
                .await
                .expect("failed to check wit files changed");
            assert!(result);
        }

        #[test]
        fn test_wit_config_with_disable_change_detection() {
            let json = r#"
            {
                "skip_fetch": true,
                "disable_change_detection": true,
                "sources": {
                    "test:package": "../local"
                }
            }
            "#;

            let config: WitConfig = serde_json::from_str(json).unwrap();
            assert!(config.skip_fetch);
            assert!(config.disable_change_detection);
            assert_eq!(config.sources.len(), 1);
            assert_eq!(config.sources["test:package"], "../local");
        }
    }
}
