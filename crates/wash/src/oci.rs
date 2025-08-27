//! OCI registry operations for pulling and pushing WebAssembly components
//!
//! This module provides functionality to interact with OCI registries for
//! WebAssembly components, including docker credential integration and
//! file-based caching.

use anyhow::{Context, Result, anyhow, bail};
use docker_credential::{CredentialRetrievalError, DockerCredential, get_credential};
use oci_client::{
    Reference,
    client::{Client, ClientConfig, ClientProtocol},
    manifest::{OciDescriptor, OciImageManifest},
    secrets::RegistryAuth,
};
use oci_wasm::{ToConfig, WASM_LAYER_MEDIA_TYPE, WasmConfig};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};
use tracing::{debug, info, instrument, warn};

use crate::inspect::decode_component;

#[allow(deprecated)]
#[deprecated = "old media type used before Wasm WG standardization"]
const WASMCLOUD_MEDIA_TYPE: &str = "application/vnd.module.wasm.content.layer.v1+wasm";
pub const OCI_CACHE_DIR: &str = "oci";

/// Configuration for OCI operations
#[derive(Debug, Default, Clone)]
pub struct OciConfig {
    /// Optional explicit credentials (username, password)
    pub credentials: Option<(String, String)>,
    /// Whether to allow insecure registries
    pub insecure: bool,
    /// Cache directory override
    pub cache_dir: Option<PathBuf>,
}

impl OciConfig {
    pub fn new_with_cache(cache_dir: PathBuf) -> Self {
        Self {
            cache_dir: Some(cache_dir),
            ..Default::default()
        }
    }
}

/// Cache manager for OCI artifacts
struct CacheManager {
    cache_dir: PathBuf,
}

impl CacheManager {
    /// Create a new cache manager with the specified or default cache directory
    fn new(cache_dir: PathBuf) -> Self {
        Self { cache_dir }
    }

    /// Get the cache path for a given OCI reference (simplified)
    fn get_cache_path(&self, reference: &str) -> PathBuf {
        // Hash for uniqueness, but keep the reference in the path for readability
        let mut hasher = Sha256::new();
        hasher.update(reference.as_bytes());
        let hash = format!("{:x}", hasher.finalize());
        let short_hash = &hash[..8];

        // Sanitize the reference for filesystem use
        let sanitized = reference.replace(['/', ':', '@'], "_");

        // Directory: <cache_dir>/<sanitized_reference>_<short_hash>/
        // File: <artifact_name>.wasm
        let dir = self.cache_dir.join(format!("{sanitized}_{short_hash}"));

        // Use the last segment as the artifact name (after last '/')
        let artifact_name = reference
            .rsplit('/')
            .next()
            .unwrap_or("artifact")
            .replace([':', '@'], "_");

        dir.join(format!("{artifact_name}.wasm"))
    }

    /// Check if an artifact is cached
    async fn is_cached(&self, reference: &str) -> bool {
        let cache_path = self.get_cache_path(reference);
        tokio::fs::metadata(&cache_path).await.is_ok()
    }

    /// Read cached artifact
    async fn read_cached(&self, reference: &str) -> Result<Vec<u8>> {
        info!(reference = %reference, "reading cached artifact instead of pulling");
        let cache_path = self.get_cache_path(reference);
        debug!(path = %cache_path.display(), "reading cached artifact");
        tokio::fs::read(&cache_path)
            .await
            .with_context(|| format!("failed to read cached artifact at {}", cache_path.display()))
    }

    /// Write artifact to cache
    async fn write_to_cache(&self, reference: &str, data: &[u8]) -> Result<()> {
        let cache_path = self.get_cache_path(reference);

        debug!(path = %cache_path.display(), "writing to cache");
        // Create parent directories
        if let Some(parent) = cache_path.parent() {
            tokio::fs::create_dir_all(parent).await.with_context(|| {
                format!("failed to create cache directory {}", parent.display())
            })?;
        }

        tokio::fs::write(&cache_path, data)
            .await
            .with_context(|| format!("failed to write to cache at {}", cache_path.display()))?;

        Ok(())
    }
}

/// Credential resolver that implements the precedence: explicit → docker creds → anonymous
struct CredentialResolver {
    explicit_credentials: Option<(String, String)>,
}

impl CredentialResolver {
    fn new(explicit_credentials: Option<(String, String)>) -> Self {
        Self {
            explicit_credentials,
        }
    }

    /// Resolve credentials for a given registry
    #[instrument(skip(self), fields(registry = %registry))]
    async fn resolve_credentials(&self, registry: &str) -> RegistryAuth {
        // First, try explicit credentials
        if let Some((username, password)) = &self.explicit_credentials {
            debug!("using explicit credentials");
            return RegistryAuth::Basic(username.clone(), password.clone());
        }

        // Next, try docker credential helper
        match self.get_docker_credentials(registry).await {
            Ok(Some(auth)) => {
                debug!("using docker credential helper");
                return auth;
            }
            Ok(None) => debug!("no docker credentials found"),
            Err(e) => warn!(error = %e, "failed to retrieve docker credentials"),
        }

        // Fall back to anonymous
        debug!("Using anonymous access");
        RegistryAuth::Anonymous
    }

    /// Attempt to retrieve credentials from docker credential helper
    async fn get_docker_credentials(&self, registry: &str) -> Result<Option<RegistryAuth>> {
        match get_credential(registry) {
            Ok(DockerCredential::UsernamePassword(user, pass)) => {
                Ok(Some(RegistryAuth::Basic(user, pass)))
            }
            Ok(DockerCredential::IdentityToken(_)) => {
                bail!("docker credential helper returned identity token, which is not supported")
            }
            Err(
                CredentialRetrievalError::ConfigNotFound
                | CredentialRetrievalError::NoCredentialConfigured,
            ) => Ok(None),
            // Edge case for Macos, shows as an error when really it's just not found
            Err(CredentialRetrievalError::HelperFailure { stdout, .. })
                if stdout.contains("credentials not found in native keychain") =>
            {
                Ok(None)
            }
            Err(e) => Err(anyhow!("docker credential retrieval error: {e}")),
        }
    }
}

/// Pull a WebAssembly component from an OCI registry
///
/// # Arguments
/// * `reference` - OCI reference (e.g., "registry.io/my/component:v1.0.0")
/// * `config` - Configuration for the pull operation
///
/// # Returns
/// Raw bytes of the WebAssembly component
#[instrument(skip(config), fields(reference = %reference))]
pub async fn pull_component(reference: &str, config: OciConfig) -> Result<Vec<u8>> {
    info!(reference = %reference, "Pulling component");

    // Parse OCI reference
    let reference_parsed = Reference::try_from(reference)
        .with_context(|| format!("invalid OCI reference: {reference}"))?;

    // Initialize cache manager
    let cache_manager = config
        .cache_dir
        .as_ref()
        .map(|dir| CacheManager::new(dir.clone()));
    if let Some(cache_manager) = &cache_manager {
        // Check cache first
        if cache_manager.is_cached(reference).await {
            debug!("Found cached artifact");
            return cache_manager.read_cached(reference).await;
        }
    }

    // Setup credential resolver
    let credential_resolver = CredentialResolver::new(config.credentials);
    let auth = credential_resolver
        .resolve_credentials(reference_parsed.registry())
        .await;

    // Configure OCI client
    let client_config = ClientConfig {
        protocol: if config.insecure {
            ClientProtocol::Http
        } else {
            ClientProtocol::Https
        },
        ..Default::default()
    };

    let client = Client::new(client_config);

    // Pull the component using oci-client
    let image_data = client
        .pull(
            &reference_parsed,
            &auth,
            vec![
                WASM_LAYER_MEDIA_TYPE,
                #[allow(deprecated)]
                WASMCLOUD_MEDIA_TYPE,
            ],
        )
        .await
        .with_context(|| format!("failed to pull component from {reference}"))?;

    // Extract the component bytes from the first layer
    let component_data = image_data
        .layers
        .first()
        .ok_or_else(|| anyhow!("no layers found in pulled artifact"))?
        .data
        .clone();

    // Validate that it's a valid WebAssembly component
    validate_component(&component_data)
        .await
        .with_context(|| "pulled artifact is not a valid WebAssembly component")?;

    // Cache the component
    if let Some(cache_manager) = &cache_manager {
        cache_manager
            .write_to_cache(reference, &component_data)
            .await
            .with_context(|| "failed to cache component")?;
    }

    info!(size = component_data.len(), "Successfully pulled component");
    Ok(component_data)
}

/// Push a WebAssembly component to an OCI registry
///
/// # Arguments
/// * `reference` - OCI reference (e.g., "registry.io/my/component:v1.0.0")
/// * `component_data` - Raw bytes of the WebAssembly component
/// * `config` - Configuration for the push operation
/// * `annotations` - Optional OCI annotations to add to the manifest
///
/// # Returns
/// The digest of the pushed component
#[instrument(
    skip(component_data, config, annotations),
    fields(
        reference = %reference,
        size = component_data.len(),
        annotation_count = annotations.as_ref().map_or(0, |a| a.len())
    )
)]
pub async fn push_component(
    reference: &str,
    component_data: &[u8],
    config: OciConfig,
    annotations: Option<HashMap<String, String>>,
) -> Result<String> {
    info!(
        reference = %reference,
        size = component_data.len(),
        "pushing component"
    );

    // Parse OCI reference
    let reference_parsed = Reference::try_from(reference)
        .with_context(|| format!("invalid OCI reference: {reference}"))?;

    // Validate the component before pushing
    validate_component(component_data)
        .await
        .with_context(|| "component data is not a valid WebAssembly component")?;

    // Setup credential resolver
    let credential_resolver = CredentialResolver::new(config.credentials);
    let auth = credential_resolver
        .resolve_credentials(reference_parsed.registry())
        .await;

    // Configure OCI client
    let client_config = ClientConfig {
        protocol: if config.insecure {
            ClientProtocol::Http
        } else {
            ClientProtocol::Https
        },
        ..Default::default()
    };

    let client = Client::new(client_config);

    // Create the WebAssembly configuration and layer using oci-wasm
    let (wasm_config, image_layer) = WasmConfig::from_raw_component(component_data.to_vec(), None)
        .with_context(|| "failed to create WebAssembly configuration from component")?;

    let layers = vec![image_layer];
    let config_obj = wasm_config
        .to_config()
        .with_context(|| "failed to convert WebAssembly config")?;

    // Create custom manifest with annotations if provided
    let manifest = annotations.filter(|a| !a.is_empty()).map(|annotations| {
        // Convert HashMap to BTreeMap for annotations
        let btree_annotations: BTreeMap<String, String> = annotations.into_iter().collect();

        // Create manifest descriptors for the config and layers
        let config_descriptor = OciDescriptor {
            media_type: config_obj.media_type.clone(),
            digest: config_obj.sha256_digest(),
            size: config_obj.data.len() as i64,
            urls: None,
            annotations: None,
        };

        let layer_descriptors: Vec<OciDescriptor> = layers
            .iter()
            .map(|layer| OciDescriptor {
                media_type: layer.media_type.clone(),
                digest: layer.sha256_digest(),
                size: layer.data.len() as i64,
                urls: None,
                annotations: None,
            })
            .collect();

        OciImageManifest {
            schema_version: 2,
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            config: config_descriptor,
            layers: layer_descriptors,
            subject: None,
            artifact_type: None,
            annotations: Some(btree_annotations),
        }
    });

    // Push the component
    client
        .push(&reference_parsed, &layers, config_obj, &auth, manifest)
        .await
        .with_context(|| format!("failed to push component to {reference}"))?;

    // Calculate the digest for return value
    let digest = format!("sha256:{:x}", Sha256::digest(component_data));

    // Cache the pushed component
    if let Some(cache_dir) = config.cache_dir {
        let cache_manager = CacheManager::new(cache_dir);
        cache_manager
            .write_to_cache(reference, component_data)
            .await
            .with_context(|| "failed to cache pushed component")?;
    }

    info!(digest = %digest, "successfully pushed component");
    Ok(digest)
}

/// Validate that the provided bytes represent a valid WebAssembly component
///
/// This function parses the WebAssembly bytes and validates that they form
/// a valid WebAssembly component or a WIT package, not just a raw module.
pub async fn validate_component(data: &[u8]) -> Result<()> {
    decode_component(data).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_cache_manager_path_generation() {
        let temp_dir = TempDir::new().unwrap();
        let cache_manager = CacheManager::new(temp_dir.path().to_path_buf());

        let reference = "localhost:5000/test:latest";
        let cache_path = cache_manager.get_cache_path(reference);

        assert!(cache_path.starts_with(temp_dir.path()));
        assert!(cache_path.extension().unwrap() == "wasm");
    }

    #[test]
    fn test_oci_config_default() {
        let config = OciConfig::default();
        assert!(config.credentials.is_none());
        assert!(!config.insecure);
        assert!(config.cache_dir.is_none());
    }

    #[tokio::test]
    async fn test_cache_manager_operations() {
        let temp_dir = TempDir::new().unwrap();
        let cache_manager = CacheManager::new(temp_dir.path().to_path_buf());

        let reference = "localhost:5000/test:v1.0.0";
        let test_data = b"test component data";

        // Should not be cached initially
        assert!(!cache_manager.is_cached(reference).await);

        // Cache the data
        cache_manager
            .write_to_cache(reference, test_data)
            .await
            .unwrap();

        // Should now be cached
        assert!(cache_manager.is_cached(reference).await);

        // Should be able to read the cached data
        let cached_data = cache_manager.read_cached(reference).await.unwrap();
        assert_eq!(cached_data, test_data);
    }

    #[tokio::test]
    async fn test_pull_and_validate_ghcr_component() {
        // Use the public OCI reference
        let references = vec![
            // wasmCloud old hello world component
            "ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0",
            // Published interface
            "ghcr.io/wasmcloud/interfaces/wasmcloud/secrets:0.1.0-draft",
            // Bytecode Alliance sample component
            "ghcr.io/bytecodealliance/sample-wasi-http-rust/sample-wasi-http-rust:latest",
        ];
        let config = OciConfig {
            credentials: None,
            insecure: false,
            cache_dir: None,
        };

        // Pull the component anonymously
        for reference in references {
            let component_bytes = pull_component(reference, config.clone())
                .await
                .expect("Failed to pull component");

            let res = validate_component(&component_bytes).await;
            assert!(
                res.is_ok(),
                "Component validation failed for {reference}: {}",
                res.unwrap_err()
            );
        }
    }

    #[test]
    fn test_oci_config_with_cache() {
        let temp_dir = TempDir::new().unwrap();
        let config = OciConfig::new_with_cache(temp_dir.path().to_path_buf());

        assert!(config.cache_dir.is_some());
        assert_eq!(config.cache_dir.unwrap(), temp_dir.path());
        assert!(config.credentials.is_none());
        assert!(!config.insecure);
    }

    #[test]
    fn test_annotations_manifest_creation() {
        // Test that annotations are properly converted and stored
        let mut annotations = HashMap::new();
        annotations.insert(
            "org.opencontainers.image.description".to_string(),
            "A test component".to_string(),
        );
        annotations.insert(
            "org.opencontainers.image.source".to_string(),
            "https://github.com/test/repo".to_string(),
        );
        annotations.insert("custom.annotation".to_string(), "custom value".to_string());

        // Convert to BTreeMap (like the code does)
        let btree_annotations: BTreeMap<String, String> = annotations.into_iter().collect();

        assert_eq!(btree_annotations.len(), 3);
        assert_eq!(
            btree_annotations.get("org.opencontainers.image.description"),
            Some(&"A test component".to_string())
        );
        assert_eq!(
            btree_annotations.get("org.opencontainers.image.source"),
            Some(&"https://github.com/test/repo".to_string())
        );
        assert_eq!(
            btree_annotations.get("custom.annotation"),
            Some(&"custom value".to_string())
        );
    }

    #[test]
    fn test_empty_annotations_handling() {
        let empty_annotations = HashMap::new();

        // Test that empty annotations don't create unnecessary structures
        assert_eq!(empty_annotations.len(), 0);

        let btree_annotations: BTreeMap<String, String> = empty_annotations.into_iter().collect();
        assert_eq!(btree_annotations.len(), 0);
    }

    #[test]
    fn test_annotation_key_value_format() {
        // Test various valid annotation formats
        let valid_annotations = vec![
            ("simple", "value"),
            (
                "org.opencontainers.image.description",
                "A longer description with spaces",
            ),
            ("custom.domain.com/annotation", "value-with-dashes"),
            ("123numeric", "123"),
            ("key_with_underscores", "value_with_underscores"),
        ];

        let mut annotations = HashMap::new();
        for (key, value) in valid_annotations {
            annotations.insert(key.to_string(), value.to_string());
        }

        assert_eq!(annotations.len(), 5);
        assert_eq!(annotations.get("simple"), Some(&"value".to_string()));
        assert_eq!(
            annotations.get("org.opencontainers.image.description"),
            Some(&"A longer description with spaces".to_string())
        );
    }

    #[test]
    fn test_standard_opencontainer_annotations() {
        // Test that standard OpenContainer annotations work as expected
        let mut annotations = HashMap::new();

        // Standard OpenContainer annotations
        annotations.insert(
            "org.opencontainers.image.description".to_string(),
            "Component description".to_string(),
        );
        annotations.insert(
            "org.opencontainers.image.source".to_string(),
            "https://github.com/example/repo".to_string(),
        );
        annotations.insert(
            "org.opencontainers.image.url".to_string(),
            "https://example.com".to_string(),
        );
        annotations.insert(
            "org.opencontainers.image.version".to_string(),
            "1.0.0".to_string(),
        );
        annotations.insert(
            "org.opencontainers.image.licenses".to_string(),
            "Apache-2.0".to_string(),
        );
        annotations.insert(
            "org.opencontainers.image.authors".to_string(),
            "John Doe <john@example.com>".to_string(),
        );

        assert_eq!(annotations.len(), 6);

        for (key, expected_value) in &annotations {
            assert!(key.starts_with("org.opencontainers.image."));
            assert!(!expected_value.is_empty());
        }
    }
}
