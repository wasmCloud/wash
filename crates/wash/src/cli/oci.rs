use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Args, Subcommand};
use etcetera::AppStrategy;
use tracing::instrument;

use crate::{
    cli::{CliContext, CommandOutput},
    oci::{OCI_CACHE_DIR, OciConfig, pull_component, push_component},
};

#[derive(Subcommand, Debug, Clone)]
pub enum OciCommand {
    Pull(PullCommand),
    Push(PushCommand),
}

impl OciCommand {
    /// Handle the OCI command
    #[instrument(level = "debug", skip_all, name = "oci")]
    pub async fn handle(self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        match self {
            OciCommand::Pull(cmd) => cmd.handle(ctx).await,
            OciCommand::Push(cmd) => cmd.handle(ctx).await,
        }
    }
}

#[derive(Args, Debug, Clone)]
pub struct PullCommand {
    /// The OCI reference to pull
    #[clap(name = "reference")]
    reference: String,
}

impl PullCommand {
    /// Handle the OCI command
    #[instrument(level = "debug", skip_all, name = "oci")]
    pub async fn handle(self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let oci_config = OciConfig::new_with_cache(ctx.cache_dir().join(OCI_CACHE_DIR));
        let c = pull_component(&self.reference, oci_config).await?;
        // Currently, this command does not perform any operations.
        // It can be extended in the future to handle OCI-related tasks.
        Ok(CommandOutput::ok(
            "OCI command executed successfully.".to_string(),
            Some(serde_json::json!({
                "message": "OCI command executed successfully.",
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
}

impl PushCommand {
    /// Handle the OCI command
    #[instrument(level = "debug", skip_all, name = "oci")]
    pub async fn handle(self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let component = tokio::fs::read(&self.component_path)
            .await
            .context("failed to read component file")?;

        // TODO: validate component?

        let oci_config = OciConfig::new_with_cache(ctx.cache_dir().join(OCI_CACHE_DIR));

        push_component(&self.reference, &component, oci_config).await?;

        Ok(CommandOutput::ok(
            "OCI command executed successfully.".to_string(),
            Some(serde_json::json!({
                "message": "OCI command executed successfully.",
                "success": true,
            })),
        ))
    }
}
