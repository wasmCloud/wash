use clap::Args;
use etcetera::AppStrategy;
use tracing::{info, instrument};

use crate::{
    cli::{CliCommand, CliContext, CommandOutput},
    inspect::{decode_component, print_component_wit},
    oci::{OCI_CACHE_DIR, OciConfig, pull_component},
};
use anyhow::Context;
use std::path::Path;

#[derive(Args, Debug, Clone)]
pub struct InspectCommand {
    /// Inspect a component by its reference, which can be a local file path or a remote OCI reference
    #[clap(name = "component_reference")]
    pub component_reference: String,
}

impl CliCommand for InspectCommand {
    #[instrument(level = "debug", skip_all, name = "inspect")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let bytes = if Path::new(&self.component_reference).exists() {
            // Load component from file
            tokio::fs::read(&self.component_reference)
                .await
                .context("Failed to read component file")?
        } else {
            info!(?self.component_reference, "component reference is not a local file path, pulling from remote");
            // Pull component from remote
            pull_component(
                &self.component_reference,
                OciConfig::new_with_cache(ctx.cache_dir().join(OCI_CACHE_DIR)),
            )
            .await
            .context("Failed to pull component")?
        };

        let component = decode_component(bytes.as_slice())
            .await
            .context("Failed to decode component")?;

        // Print the component WIT
        let wit = print_component_wit(component)
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
