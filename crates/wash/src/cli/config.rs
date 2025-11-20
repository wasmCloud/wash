use crate::{
    cli::{CliCommand, CliContext, CommandOutput},
    config::{Config, generate_default_config, local_config_path},
};
use anyhow::Context as _;
use clap::Subcommand;
use std::path::PathBuf;
use tracing::instrument;
use url::Url;

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
    /// Validate the current configuration
    #[clap(group = clap::ArgGroup::new("validate_targets")
        .required(true)
        .multiple(false))]
    Validate {
        /// Path to specific config file to validate (optional)
        #[clap(long, group = "validate_targets")]
        file: Option<PathBuf>,
        /// Validate project config instead of global config
        #[clap(long, group = "validate_targets")]
        project: bool,
    },
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

                // Global configuration should include templates, but local shouldn't by default
                generate_default_config(&config_path, *force, *global)
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
            ConfigCommand::Validate { file, project } => {
                let validation_path: PathBuf = match file {
                    Some(path) => path.clone(),
                    None => {
                        if *project {
                            local_config_path(
                                &std::env::current_dir().context("failed to get current dir")?,
                            )
                        } else {
                            ctx.config_path()
                        }
                    }
                };

                if !validation_path.exists() {
                    return Ok(CommandOutput::error(
                        format!(
                            "No configuration file found at {}",
                            validation_path.display()
                        ),
                        Some(serde_json::json!({
                             "message": "Configuration file not found.",
                             "success": false,
                        })),
                    ));
                };

                let content = std::fs::read_to_string(&validation_path).context(format!(
                    "Failed to read file: {}",
                    validation_path.display()
                ))?;

                let config: Config = match serde_json::from_str(&content) {
                    Ok(config) => config,
                    Err(e) => {
                        return Ok(CommandOutput::error(
                            format!("Json is NOT valid and cannot be parsed. With error {e}"),
                            Some(serde_json::json!({
                            "message": "Json is NOT valid and cannot be parsed.",
                             "success": false,

                             })),
                        ));
                    }
                };

                for template in config.templates {
                    match Url::parse(&template.repository) {
                        Ok(_) => {}
                        Err(_) => {
                            return Ok(CommandOutput::error(
                                format!(
                                    "The repository Url is NOT valid for template {}",
                                    template.name
                                ),
                                Some(serde_json::json!({
                                "message": format!("The repository Url is NOT valid for template {}", template.name),
                                 "success": false,
                                 })),
                            ));
                        }
                    }
                }
                if let Some(wit_config) = config.wit {
                    for reg in wit_config.registries {
                        match Url::parse(&reg.url) {
                            Ok(_) => {}
                            Err(_) => {
                                return Ok(CommandOutput::error(
                                    format!("The wit registry {} is not a valid Url", reg.url),
                                    Some(serde_json::json!({
                                    "message": format!("The wit registry {} is not a valid Url", reg.url),
                                     "success": false,
                                     })),
                                ));
                            }
                        }
                    }
                    if let Some(dir_path) = &wit_config.wit_dir
                        && !dir_path.exists()
                    {
                        return Ok(CommandOutput::error(
                            format!("Wit directory {} does not exist", dir_path.display()),
                            Some(serde_json::json!({
                            "message": format!("Wit directory {} does not exist", dir_path.display()),
                             "success": false,
                             })),
                        ));
                    }
                }
                let success_message =
                    format!("Configuration file is valid: {}", validation_path.display());

                Ok(CommandOutput::ok(
                    &success_message,
                    Some(serde_json::json!({
                        "message": success_message,
                        "success": true,
                    })),
                ))
            }
        }
    }
}
