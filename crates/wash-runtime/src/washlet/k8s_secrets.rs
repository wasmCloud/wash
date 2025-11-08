//! Kubernetes Secret Resolution
//!
//! This module provides functionality to resolve Kubernetes secret references
//! to actual credentials that can be used for OCI registry authentication.
//!
//! # Supported Secret Types
//!
//! 1. **kubernetes.io/dockerconfigjson** - Docker registry secrets
//!    - Contains a `.dockerconfigjson` key with Docker config format
//!    - Standard Kubernetes format for registry credentials
//!
//! 2. **Opaque** - Generic secrets with custom keys
//!    - Expects `username` and `password` keys
//!    - Simpler format for basic auth
//!
//! # Security
//!
//! - Secrets are fetched from the Kubernetes API using in-cluster configuration
//! - Requires appropriate RBAC permissions to read secrets in the namespace
//! - Secret data is base64-decoded but kept as plain String (see OciConfig security notes)

use anyhow::{Context, Result};
use base64::Engine;
use k8s_openapi::api::core::v1::Secret;
use kube::{Api, Client};
use tracing::{debug, warn};

/// Kubernetes secret type for Docker registry credentials
const DOCKER_CONFIG_JSON_TYPE: &str = "kubernetes.io/dockerconfigjson";

/// Key name for Docker config JSON in secrets
const DOCKER_CONFIG_JSON_KEY: &str = ".dockerconfigjson";

/// Resolve a Kubernetes secret reference to username/password credentials
///
/// # Arguments
///
/// * `secret_name` - Name of the secret to resolve
/// * `namespace` - Kubernetes namespace containing the secret
///
/// # Returns
///
/// Returns `Ok(Some((username, password)))` if credentials were successfully resolved.
/// Returns `Ok(None)` if the secret exists but credentials couldn't be extracted.
/// Returns `Err` if there was an error accessing Kubernetes or the secret doesn't exist.
///
/// # Errors
///
/// - Kubernetes API errors (network, auth, not found, etc.)
/// - Secret data parsing errors
/// - Missing required keys in secret data
///
/// # Example
///
/// ```ignore
/// let (username, password) = resolve_secret("my-registry-secret", "default")
///     .await?
///     .ok_or_else(|| anyhow!("Failed to extract credentials from secret"))?;
/// ```
#[tracing::instrument(skip_all, fields(secret_name = %secret_name, namespace = %namespace))]
pub async fn resolve_secret(
    secret_name: &str,
    namespace: &str,
) -> Result<Option<(String, String)>> {
    debug!("Resolving Kubernetes secret");

    // Create Kubernetes client with in-cluster configuration
    let client = Client::try_default().await.with_context(
        || "failed to create Kubernetes client - ensure running in-cluster with proper RBAC",
    )?;

    // Create API handle for secrets in the specified namespace
    let secrets: Api<Secret> = Api::namespaced(client, namespace);

    // Fetch the secret
    let secret = secrets.get(secret_name).await.with_context(|| {
        format!(
            "failed to get secret '{}' in namespace '{}'",
            secret_name, namespace
        )
    })?;

    // Extract credentials based on secret type
    match secret.type_.as_deref() {
        Some(DOCKER_CONFIG_JSON_TYPE) => parse_docker_config_json_secret(&secret),
        Some("Opaque") | None => parse_opaque_secret(&secret),
        Some(other_type) => {
            warn!(secret_type = %other_type, "Unsupported secret type, attempting to parse as opaque");
            parse_opaque_secret(&secret)
        }
    }
}

/// Parse a kubernetes.io/dockerconfigjson secret
///
/// This extracts credentials from a Docker config JSON secret.
/// For simplicity, we extract the first registry's credentials found.
///
/// # Format
///
/// ```json
/// {
///   "auths": {
///     "registry.io": {
///       "username": "user",
///       "password": "pass",
///       "auth": "base64(user:pass)"  // optional
///     }
///   }
/// }
/// ```
fn parse_docker_config_json_secret(secret: &Secret) -> Result<Option<(String, String)>> {
    let data = secret.data.as_ref().context("secret has no data field")?;

    let config_bytes = data
        .get(DOCKER_CONFIG_JSON_KEY)
        .context("secret missing .dockerconfigjson key")?;

    // Decode from base64 (Kubernetes stores secret data as base64)
    let config_json = String::from_utf8(config_bytes.0.clone())
        .with_context(|| "failed to decode .dockerconfigjson as UTF-8")?;

    // Parse JSON
    let config: serde_json::Value = serde_json::from_str(&config_json)
        .with_context(|| "failed to parse .dockerconfigjson as JSON")?;

    // Extract auths object
    let auths = config
        .get("auths")
        .and_then(|v| v.as_object())
        .context(".dockerconfigjson missing 'auths' object")?;

    // Get first registry's credentials
    for (registry, auth_data) in auths {
        debug!(registry = %registry, "Found registry in Docker config");

        // Try to get username and password directly
        if let (Some(username), Some(password)) = (
            auth_data.get("username").and_then(|v| v.as_str()),
            auth_data.get("password").and_then(|v| v.as_str()),
        ) {
            return Ok(Some((username.to_string(), password.to_string())));
        }

        // Try to decode from "auth" field (base64-encoded "username:password")
        if let Some(auth) = auth_data.get("auth").and_then(|v| v.as_str())
            && let Ok(decoded) = base64::prelude::BASE64_STANDARD.decode(auth)
            && let Ok(credentials) = String::from_utf8(decoded)
            && let Some((username, password)) = credentials.split_once(':')
        {
            return Ok(Some((username.to_string(), password.to_string())));
        }
    }

    warn!("Docker config JSON secret has no extractable credentials");
    Ok(None)
}

/// Parse an Opaque secret with username/password keys
///
/// Expects the secret to have `username` and `password` keys in its data.
fn parse_opaque_secret(secret: &Secret) -> Result<Option<(String, String)>> {
    let data = secret.data.as_ref().context("secret has no data field")?;

    let username_bytes = data
        .get("username")
        .context("secret missing 'username' key")?;

    let password_bytes = data
        .get("password")
        .context("secret missing 'password' key")?;

    let username = String::from_utf8(username_bytes.0.clone())
        .with_context(|| "failed to decode username as UTF-8")?;

    let password = String::from_utf8(password_bytes.0.clone())
        .with_context(|| "failed to decode password as UTF-8")?;

    if username.is_empty() || password.is_empty() {
        warn!("Secret has empty username or password");
        return Ok(None);
    }

    Ok(Some((username, password)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::ByteString;
    use std::collections::BTreeMap;

    #[test]
    fn test_parse_opaque_secret() {
        let mut data = BTreeMap::new();
        data.insert(
            "username".to_string(),
            ByteString("testuser".as_bytes().to_vec()),
        );
        data.insert(
            "password".to_string(),
            ByteString("testpass".as_bytes().to_vec()),
        );

        let secret = Secret {
            data: Some(data),
            type_: Some("Opaque".to_string()),
            ..Default::default()
        };

        let result = parse_opaque_secret(&secret).unwrap();
        assert_eq!(
            result,
            Some(("testuser".to_string(), "testpass".to_string()))
        );
    }

    #[test]
    fn test_parse_opaque_secret_empty_password() {
        let mut data = BTreeMap::new();
        data.insert(
            "username".to_string(),
            ByteString("testuser".as_bytes().to_vec()),
        );
        data.insert("password".to_string(), ByteString("".as_bytes().to_vec()));

        let secret = Secret {
            data: Some(data),
            type_: Some("Opaque".to_string()),
            ..Default::default()
        };

        let result = parse_opaque_secret(&secret).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_docker_config_json_with_username_password() {
        let config = r#"{
            "auths": {
                "ghcr.io": {
                    "username": "myuser",
                    "password": "mypass"
                }
            }
        }"#;

        let mut data = BTreeMap::new();
        data.insert(
            ".dockerconfigjson".to_string(),
            ByteString(config.as_bytes().to_vec()),
        );

        let secret = Secret {
            data: Some(data),
            type_: Some("kubernetes.io/dockerconfigjson".to_string()),
            ..Default::default()
        };

        let result = parse_docker_config_json_secret(&secret).unwrap();
        assert_eq!(result, Some(("myuser".to_string(), "mypass".to_string())));
    }

    #[test]
    fn test_parse_docker_config_json_with_auth_field() {
        // "myuser:mypass" in base64
        let auth_base64 = base64::prelude::BASE64_STANDARD.encode("myuser:mypass");

        let config = format!(
            r#"{{
                "auths": {{
                    "ghcr.io": {{
                        "auth": "{}"
                    }}
                }}
            }}"#,
            auth_base64
        );

        let mut data = BTreeMap::new();
        data.insert(
            ".dockerconfigjson".to_string(),
            ByteString(config.as_bytes().to_vec()),
        );

        let secret = Secret {
            data: Some(data),
            type_: Some("kubernetes.io/dockerconfigjson".to_string()),
            ..Default::default()
        };

        let result = parse_docker_config_json_secret(&secret).unwrap();
        assert_eq!(result, Some(("myuser".to_string(), "mypass".to_string())));
    }
}
