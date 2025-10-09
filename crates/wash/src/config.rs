//! Contains the [Config] struct and related functions for managing
//! wash configuration, including loading, saving, and merging configurations
//! with explicit defaults.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use figment::{
    Figment,
    providers::{Env, Format, Json},
};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::{
    cli::CONFIG_FILE_NAME,
    component_build::{
        BuildConfig, ProjectType, RustBuildConfig, TinyGoBuildConfig, TypeScriptBuildConfig,
    },
    new::NewTemplate,
    wit::WitConfig,
};

pub const PROJECT_CONFIG_DIR: &str = ".wash";

/// Main wash configuration structure with hierarchical merging support and explicit defaults
///
/// The "global" [Config] is stored under the user's XDG_CONFIG_HOME directory
/// (typically `~/.config/wash/config.json`), while the "local" project configuration
/// is stored in the project's `.wash/config.json` file. This allows for both reasonable
/// global defaults and project-specific overrides.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Config {
    /// Build configuration for different project types (default: empty/optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildConfig>,

    /// Template configuration for new project creation (default: wasmCloud templates)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub templates: Vec<NewTemplate>,

    /// WIT dependency management configuration (default: empty/optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wit: Option<WitConfig>,
    // TODO(#15): Support dev config which can be overridden in local project config
    // e.g. for runtime config, http ports, etc
}

impl Config {
    /// Create a new [Config] instance with default values and the list of wasmCloud templates
    pub fn default_with_templates() -> Self {
        Self {
            templates: vec![
                NewTemplate {
                    name: "cosmonic-control-welcome-tour".to_string(),
                    description: Some("Welcome to Cosmonic!".to_string()),
                    repository: "https://github.com/cosmonic-labs/control-demos".to_string(),
                    subfolder: Some("welcome-tour".to_string()),
                    language: crate::new::TemplateLanguage::TypeScript,
                    git_ref: None,
                },
                NewTemplate {
                    name: "sample-wasi-http-rust".to_string(),
                    description: Some(
                        "An example wasi:http server component written in Rust".to_string(),
                    ),
                    repository: "https://github.com/bytecodealliance/sample-wasi-http-rust"
                        .to_string(),
                    subfolder: None,
                    language: crate::new::TemplateLanguage::Rust,
                    git_ref: None,
                },
                NewTemplate {
                    name: "sample-wasi-http-js".to_string(),
                    description: Some(
                        "An example wasi:http server component written in JavaScript".to_string(),
                    ),
                    repository: "https://github.com/bytecodealliance/sample-wasi-http-js"
                        .to_string(),
                    subfolder: None,
                    language: crate::new::TemplateLanguage::TypeScript,
                    git_ref: None,
                },
                NewTemplate {
                    name: "http-hello-world".to_string(),
                    description: Some("A simple HTTP hello world component".to_string()),
                    repository: "https://github.com/wasmcloud/wasmcloud".to_string(),
                    subfolder: Some("examples/rust/components/http-hello-world".to_string()),
                    language: crate::new::TemplateLanguage::Rust,
                    git_ref: None,
                },
                NewTemplate {
                    name: "http-hello-world".to_string(),
                    description: Some("A simple HTTP hello world component".to_string()),
                    repository: "https://github.com/wasmcloud/wasmcloud".to_string(),
                    subfolder: Some("examples/tinygo/components/http-hello-world".to_string()),
                    language: crate::new::TemplateLanguage::TinyGo,
                    git_ref: None,
                },
                NewTemplate {
                    name: "http-hello-world".to_string(),
                    description: Some("A simple HTTP hello world component".to_string()),
                    repository: "https://github.com/wasmcloud/typescript".to_string(),
                    subfolder: Some("examples/components/http-hello-world".to_string()),
                    language: crate::new::TemplateLanguage::TypeScript,
                    git_ref: None,
                },
            ],
            ..Default::default()
        }
    }
}

/// Load configuration with hierarchical merging
/// Order of precedence (lowest to highest):
/// 1. Default values
/// 2. Global config (~/.wash/config.json)
/// 3. Local project config (.wash/config.json)
/// 4. Environment variables (WASH_ prefix)
/// 5. Command line arguments
///
/// # Arguments
/// - `global_config_path`:
pub fn load_config<T>(
    global_config_path: &Path,
    project_dir: Option<&Path>,
    cli_args: Option<T>,
) -> Result<Config>
where
    T: Serialize + Into<Config>,
{
    let mut figment = Figment::new();

    // Start with defaults
    figment = figment.merge(figment::providers::Serialized::defaults(Config::default()));

    // Global config file
    if global_config_path.exists() {
        figment = figment.merge(Json::file(global_config_path));
    }

    // Local project config
    if let Some(project_dir) = project_dir {
        let local_config_path = project_dir.join(PROJECT_CONFIG_DIR).join(CONFIG_FILE_NAME);
        if local_config_path.exists() {
            figment = figment.merge(Json::file(local_config_path));
        }
    }

    // Environment variables with WASH_ prefix
    figment = figment.merge(Env::prefixed("WASH_").split("__"));

    // TODO(#16): There's more testing to be done here to ensure that CLI args can override existing
    // config without replacing present values with empty values.
    if let Some(args) = cli_args {
        // Convert CLI args to configuration format
        let cli_config: Config = args.into();
        figment = figment.merge(figment::providers::Serialized::defaults(cli_config));
    }

    figment
        .extract()
        .context("Failed to load wash configuration")
}

/// Save configuration to specified path
pub async fn save_config(config: &Config, path: &Path) -> Result<()> {
    // Ensure directory exists
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await.with_context(|| {
            format!(
                "Failed to create config directory: {parent}",
                parent = parent.display()
            )
        })?;
    }

    let json = serde_json::to_string_pretty(config).context("Failed to serialize configuration")?;

    tokio::fs::write(path, json)
        .await
        .with_context(|| format!("failed to write config file: {}", path.display()))?;

    Ok(())
}

/// Generate project-specific configuration after successful build
pub async fn generate_project_config<T>(
    project_dir: &Path,
    project_type: &ProjectType,
    build_args: T,
) -> Result<()>
where
    T: Serialize,
{
    let config_dir = project_dir.join(".wash");
    let config_path = config_dir.join("config.json");

    // Don't overwrite existing config
    if config_path.exists() {
        return Ok(());
    }

    let mut config = Config::default();

    // Create a figment from the build args and extract relevant config
    let figment = Figment::new().merge(figment::providers::Serialized::defaults(build_args));

    // Try to extract build configuration from the CLI args
    match project_type {
        ProjectType::Rust => {
            if let Ok(rust_config) = figment.extract::<RustBuildConfig>() {
                config.build = Some(BuildConfig {
                    rust: Some(rust_config),
                    ..Default::default()
                });
            }
        }
        ProjectType::Go => {
            if let Ok(tinygo_config) = figment.extract::<TinyGoBuildConfig>() {
                config.build = Some(BuildConfig {
                    tinygo: Some(tinygo_config),
                    ..Default::default()
                });
            }
        }
        ProjectType::TypeScript => {
            if let Ok(ts_config) = figment.extract::<TypeScriptBuildConfig>() {
                config.build = Some(BuildConfig {
                    typescript: Some(ts_config),
                    ..Default::default()
                });
            }
        }
        ProjectType::Unknown => {
            // Unknown project type, skip config generation
            return Ok(());
        }
    }

    save_config(&config, &config_path).await?;

    info!(
        "Generated project configuration at {}",
        config_path.display()
    );
    Ok(())
}

/// Get the local project configuration file path
pub fn local_config_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".wash").join(CONFIG_FILE_NAME)
}

/// Generate a default configuration file with all explicit defaults
/// This is useful for `wash config init` command
pub async fn generate_default_config(
    path: &Path,
    force: bool,
    include_templates: bool,
) -> Result<()> {
    // Don't overwrite existing config unless force is specified
    if path.exists() && !force {
        bail!(
            "Configuration file already exists at {}. Use --force to overwrite",
            path.display()
        );
    }

    let default_config = if include_templates {
        Config::default_with_templates()
    } else {
        Config::default()
    };
    save_config(&default_config, path).await?;

    info!(config_path = %path.display(), "Generated default configuration");
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::cli::test::create_test_cli_context;

    use figment::Jail;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_load_config_only_defaults() -> anyhow::Result<()> {
        let ctx = create_test_cli_context().await?;
        let config = load_config(&ctx.config_path(), None, None::<Config>)?;
        assert_eq!(config, Config::default());
        Ok(())
    }

    #[tokio::test]
    async fn test_load_config_with_global_config() -> anyhow::Result<()> {
        let ctx = create_test_cli_context().await?;

        let global_config = Config::default_with_templates();
        save_config(&global_config, &ctx.config_path()).await?;

        let config = load_config(&ctx.config_path(), None, None::<Config>)?;
        assert_eq!(config, global_config);
        Ok(())
    }

    #[tokio::test]
    async fn test_load_config_with_local_config() -> anyhow::Result<()> {
        let ctx = create_test_cli_context().await?;

        let global_config = Config::default_with_templates();
        save_config(&global_config, &ctx.config_path()).await?;

        let project = tempdir()?;
        let project_dir = project.path();
        let local_config_file = project_dir.join(PROJECT_CONFIG_DIR).join(CONFIG_FILE_NAME);
        let mut local_config = Config::default_with_templates();
        local_config.build = Some(BuildConfig {
            rust: Some(RustBuildConfig {
                release: true,
                ..Default::default()
            }),
            ..Default::default()
        }); // Should override global
        local_config.templates = Vec::new(); // should take templates from global
        save_config(&local_config, &local_config_file).await?;

        let config = load_config(&ctx.config_path(), Some(&project_dir), None::<Config>)?;
        assert_eq!(config.wit, Config::default().wit);
        assert_eq!(config.templates, global_config.templates);
        assert_eq!(config.build, local_config.build);
        Ok(())
    }

    #[tokio::test]
    async fn test_load_config_with_env_vars() -> anyhow::Result<()> {
        let ctx = create_test_cli_context().await?;

        let project = tempdir()?;
        let project_dir = project.path();
        let local_config_file = project_dir.join(PROJECT_CONFIG_DIR).join(CONFIG_FILE_NAME);
        let mut local_config = Config::default_with_templates();
        local_config.build = Some(BuildConfig {
            rust: Some(RustBuildConfig {
                release: true,
                ..Default::default()
            }),
            ..Default::default()
        });
        save_config(&local_config, &local_config_file).await?;

        Jail::expect_with(|jail| {
            // Should override whatever was set in local configuration
            jail.set_env("WASH_BUILD__RUST__RELEASE", "false");
            // Using double underscore as delimiter allows to use multi-words for configuration via
            // env variables
            jail.set_env("WASH_BUILD__RUST__CUSTOM_COMMAND", "[cargo,build]");

            let config = load_config(&ctx.config_path(), Some(&project_dir), None::<Config>)
                .expect("configuration should be loadable");

            let rust_build_config = config
                .clone()
                .build
                .ok_or("build config should contain information")?
                .rust
                .ok_or("rust build config should contain information")?;

            assert_eq!(rust_build_config.release, false);

            assert_eq!(
                rust_build_config.custom_command,
                Some(vec!["cargo".into(), "build".into()])
            );

            Ok(())
        });
        Ok(())
    }

    #[tokio::test]
    async fn test_load_config_with_cli_args() -> anyhow::Result<()> {
        let ctx = create_test_cli_context().await?;

        let some_path = "/this/is/some/path";
        let custom_command = vec!["cargo".into(), "component".into(), "bindings".into()];
        let mut cli_config = Config::default_with_templates();
        cli_config.build = Some(BuildConfig {
            component_path: Some(some_path.into()),
            rust: Some(RustBuildConfig {
                custom_command: Some(custom_command.clone()),
                ..Default::default()
            }),
            ..Default::default()
        });

        Jail::expect_with(|jail| {
            // Should be irrelevant, as is overwritten by CLI configuration.
            jail.set_env("WASH_BUILD__RUST__CUSTOM_COMMAND", "[cargo,build]");

            let config = load_config(&ctx.config_path(), None, Some(cli_config))
                .expect("configuration should be loadable");

            let build_config = config
                .clone()
                .build
                .ok_or("build config should contain information")?;

            assert_eq!(
                build_config
                    .clone()
                    .rust
                    .ok_or("rust build config should contain information")?
                    .custom_command,
                Some(custom_command)
            );

            assert_eq!(build_config.clone().component_path, Some(some_path.into()));

            assert_eq!(
                build_config
                    .clone()
                    .rust
                    .ok_or("rust build config should contain information")?
                    .release,
                RustBuildConfig::default().release
            );

            Ok(())
        });
        Ok(())
    }
}
