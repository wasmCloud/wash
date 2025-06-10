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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Build configuration for different project types (default: empty/optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildConfig>,

    /// Template configuration for new project creation (default: wasmCloud templates)
    #[serde(default)]
    pub templates: Vec<NewTemplate>,

    /// WIT dependency management configuration (default: empty/optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wit: Option<WitConfig>,
    // TODO: Support dev config which can be overridden in local project config
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
                    repository: "https://github.com/wasmcloud/wasmcloud".to_string(),
                    subfolder: Some("examples/typescript/components/http-hello-world".to_string()),
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
    figment = figment.merge(Env::prefixed("WASH_"));

    // CLI arguments (if provided)
    if let Some(args) = cli_args {
        // Convert CLI args to configuration format
        let cli_config = args.into();
        figment = figment.merge(figment::providers::Serialized::defaults(cli_config));
    }

    figment
        .extract()
        .context("Failed to load wash configuration")
}

/// Save configuration to specified path
pub fn save_config(config: &Config, path: &Path) -> Result<()> {
    // Ensure directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create config directory: {parent}",
                parent = parent.display()
            )
        })?;
    }

    let json = serde_json::to_string_pretty(config).context("Failed to serialize configuration")?;

    std::fs::write(path, json)
        .with_context(|| format!("failed to write config file: {}", path.display()))?;

    Ok(())
}

/// Generate project-specific configuration after successful build
pub fn generate_project_config<T>(
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

    save_config(&config, &config_path)?;

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
pub fn generate_default_config(path: &Path, force: bool) -> Result<()> {
    // Don't overwrite existing config unless force is specified
    if path.exists() && !force {
        bail!(
            "Configuration file already exists at {}. Use --force to overwrite",
            path.display()
        );
    }

    let default_config = Config::default_with_templates();
    save_config(&default_config, path)?;

    info!(config_path = %path.display(), "Generated default configuration");
    Ok(())
}
