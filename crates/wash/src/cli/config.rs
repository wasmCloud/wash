use anyhow::Context as _;
use clap::Subcommand;
use etcetera::AppStrategy as _;
use tracing::instrument;

use crate::{
    cli::{CliContext, CommandOutput},
    config::generate_default_config,
};

/// Create a new component project from a template, git repository, or local path
#[derive(Subcommand, Debug, Clone)]
pub enum ConfigCommand {
    /// Initialize a new configuration file for wash
    Init {
        #[clap(long)]
        /// Overwrite existing configuration
        force: bool,
    },
    /// Print the current version and local directories used by wash
    Info {},
    /// Print the current configuration file for wash
    Show {},
    // TODO: validate config command
    // TODO: cleanup config command, to clean the dirs we use
}

impl ConfigCommand {
    #[instrument(level = "debug", skip_all, name = "config")]
    pub async fn handle(self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        match self {
            ConfigCommand::Init { force } => {
                let config_path = ctx.config_path();
                generate_default_config(&config_path, force)
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
                        "wash version: {}\nData directory: {}\nCache directory: {}\nConfig directory: {}\nConfig path: {}",
                        version, data_dir, cache_dir, config_dir, config_path
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
                let config = ctx.ensure_config().context("failed to load config")?;
                Ok(CommandOutput::ok(
                    serde_json::to_string_pretty(&config).context("failed to serialize config")?,
                    Some(serde_json::to_value(&config).context("failed to serialize config")?),
                ))
            }
        }
    }
}
