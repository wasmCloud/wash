use std::{collections::HashMap, path::PathBuf};

use anyhow::Context as _;
use clap::{Args, Subcommand};
use etcetera::AppStrategy;
use tracing::instrument;

use crate::{
    cli::{CliCommand, CliContext, CommandOutput},
    oci::{OCI_CACHE_DIR, OciConfig, pull_component, push_component},
    runtime::bindings::plugin::wasmcloud::wash::types::HookType,
};

/// Parse annotation in key=value format
fn parse_annotation(s: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = s.splitn(2, '=').collect();
    if parts.len() != 2 || parts[0].is_empty() {
        return Err("Annotation must be in key=value format".to_string());
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

#[derive(Subcommand, Debug, Clone)]
pub enum OciCommand {
    Pull(PullCommand),
    Push(PushCommand),
}

impl CliCommand for OciCommand {
    /// Handle the OCI command
    #[instrument(level = "debug", skip_all, name = "oci")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        match self {
            OciCommand::Pull(cmd) => cmd.handle(ctx).await,
            OciCommand::Push(cmd) => cmd.handle(ctx).await,
        }
    }
    fn enable_pre_hook(&self) -> Option<HookType> {
        match self {
            OciCommand::Pull(_) => None,
            OciCommand::Push(_) => Some(HookType::BeforePush),
        }
    }
    fn enable_post_hook(&self) -> Option<HookType> {
        match self {
            OciCommand::Pull(_) => None,
            OciCommand::Push(_) => Some(HookType::AfterPush),
        }
    }
}

#[derive(Args, Debug, Clone)]
pub struct PullCommand {
    /// The OCI reference to pull
    #[clap(name = "reference")]
    reference: String,
    /// The path to write the pulled component to
    #[clap(name = "component_path", default_value = "component.wasm")]
    component_path: PathBuf,
}

impl PullCommand {
    /// Handle the OCI command
    #[instrument(level = "debug", skip_all, name = "oci")]
    pub async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let oci_config = OciConfig::new_with_cache(ctx.cache_dir().join(OCI_CACHE_DIR));
        let c = pull_component(&self.reference, oci_config).await?;

        // Write the component to the specified output path
        tokio::fs::write(&self.component_path, &c)
            .await
            .context("failed to write pulled component to output path")?;

        Ok(CommandOutput::ok(
            format!(
                "Pulled and saved component to {}",
                self.component_path.display()
            ),
            Some(serde_json::json!({
                "message": "OCI command executed successfully.",
                "output_path": self.component_path.to_string_lossy(),
                "bytes": c.len(),
                "success": true,
            })),
        ))
    }
}

#[derive(Args, Debug, Clone)]
pub struct PushCommand {
    /// The OCI reference to push
    #[clap(name = "reference")]
    reference: String,
    /// The path to the component to push
    #[clap(name = "component_path")]
    component_path: PathBuf,
    /// Add an OCI annotation to the image manifest (can be specified multiple times)
    #[clap(long = "annotation", value_parser = parse_annotation)]
    annotations: Vec<(String, String)>,
}

impl PushCommand {
    /// Handle the OCI command
    #[instrument(level = "debug", skip_all, name = "oci")]
    pub async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let component = tokio::fs::read(&self.component_path)
            .await
            .context("failed to read component file")?;

        // Build annotations from explicit annotations
        let all_annotations: HashMap<String, String> = self
            .annotations
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();

        let oci_config = OciConfig::new_with_cache(ctx.cache_dir().join(OCI_CACHE_DIR));

        let digest = push_component(
            &self.reference,
            &component,
            oci_config,
            if all_annotations.is_empty() {
                None
            } else {
                Some(all_annotations)
            },
        )
        .await?;

        Ok(CommandOutput::ok(
            format!("Successfully pushed component\ndigest: {}", digest),
            Some(serde_json::json!({
                "message": "OCI command executed successfully.",
                "digest": digest,
                "success": true,
            })),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_annotation_valid() {
        // Test valid key=value format
        let result = parse_annotation("key=value");
        assert!(result.is_ok());
        let (key, value) = result.unwrap();
        assert_eq!(key, "key");
        assert_eq!(value, "value");
    }

    #[test]
    fn test_parse_annotation_with_equals_in_value() {
        // Test key=value where value contains equals sign
        let result = parse_annotation("url=http://example.com/path?param=value");
        assert!(result.is_ok());
        let (key, value) = result.unwrap();
        assert_eq!(key, "url");
        assert_eq!(value, "http://example.com/path?param=value");
    }

    #[test]
    fn test_parse_annotation_opencontainer_format() {
        // Test OpenContainer annotation format
        let result = parse_annotation("org.opencontainers.image.description=A test component");
        assert!(result.is_ok());
        let (key, value) = result.unwrap();
        assert_eq!(key, "org.opencontainers.image.description");
        assert_eq!(value, "A test component");
    }

    #[test]
    fn test_parse_annotation_empty_value() {
        // Test annotation with empty value
        let result = parse_annotation("key=");
        assert!(result.is_ok());
        let (key, value) = result.unwrap();
        assert_eq!(key, "key");
        assert_eq!(value, "");
    }

    #[test]
    fn test_parse_annotation_invalid_format() {
        // Test invalid format (no equals sign)
        let result = parse_annotation("just-a-key");
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "Annotation must be in key=value format"
        );
    }

    #[test]
    fn test_parse_annotation_only_equals() {
        // Test invalid format (only equals sign)
        let result = parse_annotation("=");
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "Annotation must be in key=value format"
        );
    }

    #[test]
    fn test_annotation_collection_and_conversion() {
        // Test that multiple annotations can be collected and converted properly
        let annotations = vec![
            parse_annotation("key1=value1").unwrap(),
            parse_annotation("key2=value2").unwrap(),
            parse_annotation("org.opencontainers.image.description=A test").unwrap(),
        ];

        let mut annotation_map = HashMap::new();
        for (key, value) in annotations {
            annotation_map.insert(key, value);
        }

        assert_eq!(annotation_map.len(), 3);
        assert_eq!(annotation_map.get("key1"), Some(&"value1".to_string()));
        assert_eq!(annotation_map.get("key2"), Some(&"value2".to_string()));
        assert_eq!(
            annotation_map.get("org.opencontainers.image.description"),
            Some(&"A test".to_string())
        );
    }
}
