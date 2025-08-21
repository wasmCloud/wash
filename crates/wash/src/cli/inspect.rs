use clap::Args;
use etcetera::AppStrategy;
use tracing::{info, instrument};

use crate::{
    cli::{CliCommand, CliContext, CommandOutput, component_build::build_component},
    inspect::{decode_component, get_component_wit},
    oci::{OCI_CACHE_DIR, OciConfig, pull_component},
};
use anyhow::Context;
use std::path::Path;

#[derive(Args, Debug, Clone)]
pub struct InspectCommand {
    /// Inspect a component by its reference, which can be a local file path, project directory, or remote OCI reference.
    /// If omitted or pointing to a directory, attempts to build and inspect a component from that directory.
    #[clap(name = "component_reference")]
    pub component_reference: Option<String>,
}

impl CliCommand for InspectCommand {
    #[instrument(level = "debug", skip_all, name = "inspect")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        // Handle the optional component reference - default to current directory if not provided
        let component_reference = match &self.component_reference {
            Some(reference) => reference.clone(),
            None => ".".to_string(),
        };

        let path = Path::new(&component_reference);

        let bytes = if path.exists() {
            if path.is_file() {
                // Direct file path - load it
                info!(?component_reference, "loading component from file");
                tokio::fs::read(&component_reference)
                    .await
                    .context("failed to read component file")?
            } else if path.is_dir() {
                // Directory - check if it's a project and build it
                info!(
                    ?component_reference,
                    "directory detected, checking if it's a project"
                );

                // Check for project files
                let project_files = [
                    "Cargo.toml",
                    "go.mod",
                    "package.json",
                    "wasmcloud.toml",
                    ".wash/config.json",
                ];
                let is_project = project_files.iter().any(|file| path.join(file).exists());

                if is_project {
                    info!(
                        ?component_reference,
                        "project directory detected, building component"
                    );

                    // Load project config and build the component
                    let config = ctx
                        .ensure_config(Some(&path))
                        .await
                        .context("Failed to load project configuration")?;

                    let build_result = build_component(&path, ctx, &config)
                        .await
                        .context("Failed to build component from project directory")?;

                    info!(artifact_path = ?build_result.artifact_path, "Component built successfully");

                    // Read the built component
                    tokio::fs::read(&build_result.artifact_path)
                        .await
                        .context("Failed to read built component file")?
                } else {
                    return Err(anyhow::anyhow!(
                        "Directory '{}' does not appear to be a project (no Cargo.toml, go.mod, package.json, wasmcloud.toml, or .wash/config.json found)",
                        component_reference
                    ));
                }
            } else {
                return Err(anyhow::anyhow!(
                    "Path '{}' exists but is neither a file nor directory",
                    component_reference
                ));
            }
        } else {
            info!(
                ?component_reference,
                "Path does not exist locally, attempting to pull from remote"
            );
            // Pull component from remote
            pull_component(
                &component_reference,
                OciConfig::new_with_cache(ctx.cache_dir().join(OCI_CACHE_DIR)),
            )
            .await
            .context("Failed to pull component from remote")?
        };

        let component = decode_component(bytes.as_slice())
            .await
            .context("Failed to decode component")?;

        // Print the component WIT
        let wit = get_component_wit(component)
            .await
            .context("Failed to print component WIT")?;

        Ok(CommandOutput::ok(
            wit.to_owned(),
            Some(serde_json::json!({
                "message": "Component inspected successfully.",
                "success": true,
                "wit": wit,
            })),
        ))
    }
}
