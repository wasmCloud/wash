use anyhow::Context as _;
use clap::Subcommand;
use tracing::instrument;

use crate::{
    cli::{CliCommand, CliContext, CommandOutput},
    config::{generate_default_config, local_config_path},
};

/// Create a new component project from a template, git repository, or local path
#[derive(Subcommand, Debug, Clone)]
pub enum ConfigCommand {
    /// Initialize a new configuration file for wash
    Init {
        #[clap(long)]
        /// Overwrite existing configuration
        force: bool,
        #[clap(long)]
        /// Overwrite global configuration instead of project
        global: bool,
    },
    /// Print the current version and local directories used by wash
    Info {},
    /// Print the current configuration file for wash
    Show {},
    // TODO(#27): validate config command
    // TODO(#29): cleanup config command, to clean the dirs we use
}

impl CliCommand for ConfigCommand {
    #[instrument(level = "debug", skip_all, name = "config")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        match self {
            ConfigCommand::Init { force, global } => {
                let config_path = if *global {
                    ctx.config_path()
                } else {
                    local_config_path(
                        &std::env::current_dir().context("failed to get current dir")?,
                    )
                };

                generate_default_config(&config_path, *force)
                    .await
                    .context("failed to initialize config")?;

                Ok(CommandOutput::ok(
                    "Configuration initialized successfully.".to_string(),
                    Some(serde_json::json!({
                        "message": "Configuration initialized successfully.",
                        "success": true,
                    })),
                ))
            }
            ConfigCommand::Info {} => {
                let version = env!("CARGO_PKG_VERSION");
                let data_dir = ctx.data_dir().display().to_string();
                let cache_dir = ctx.cache_dir().display().to_string();
                let config_dir = ctx.config_dir().display().to_string();
                let config_path = ctx.config_path().display().to_string();

                Ok(CommandOutput::ok(
                    format!(
                        "wash version: {version}\nData directory: {data_dir}\nCache directory: {cache_dir}\nConfig directory: {config_dir}\nConfig path: {config_path}"
                    ),
                    Some(serde_json::json!({
                        "version": version,
                        "data_dir": data_dir,
                        "cache_dir": cache_dir,
                        "config_dir": config_dir,
                        "config_path": config_path,
                    })),
                ))
            }
            ConfigCommand::Show {} => {
                let config = ctx
                    .ensure_config(None)
                    .await
                    .context("failed to load config")?;
                Ok(CommandOutput::ok(
                    serde_json::to_string_pretty(&config).context("failed to serialize config")?,
                    Some(serde_json::to_value(&config).context("failed to serialize config")?),
                ))
            }
        }
    }
}
