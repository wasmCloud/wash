use anyhow::{anyhow, Result};
use config::Config;
use semver::Version;
use std::path::PathBuf;

#[derive(serde::Deserialize, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LanguageConfig {
    Rust(RustConfig),
    TinyGo(TinyGoConfig),
}

#[derive(serde::Deserialize, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TypeConfig {
    Actor(ActorConfig),
    Provider(ProviderConfig),
    Interface(InterfaceConfig),
}

#[derive(serde::Deserialize, Debug)]
pub struct ProjectConfig {
    pub language: LanguageConfig,
    #[serde(rename = "type")]
    pub project_type: TypeConfig,
    pub name: String,
    pub version: Version,
}

#[derive(serde::Deserialize, Debug, PartialEq)]
pub struct ActorConfig {
    pub claims: Option<Vec<String>>,
    pub registry: Option<String>,
    pub push_insecure: bool,
    pub key_directory: Option<PathBuf>,
    pub filename: Option<String>,
    pub wasm_type: Option<String>,
}

#[derive(serde::Deserialize, Debug, PartialEq)]
pub struct ProviderConfig {
    pub capability_id: String,
    pub vendor: Option<String>,
}

#[derive(serde::Deserialize, Debug, PartialEq)]
pub struct InterfaceConfig {}

#[derive(serde::Deserialize, Debug, PartialEq)]
pub struct RustConfig {
    pub cargo_path: Option<PathBuf>,
    pub target_path: Option<PathBuf>,
}

#[derive(serde::Deserialize, Debug)]
struct RawProjectConfig {
    pub language: String,
    #[serde(rename = "type")]
    pub project_type: String,
    pub name: String,
    pub version: Version,
    pub actor: Option<ActorConfig>,
    pub provider: Option<ProviderConfig>,
    pub rust: Option<RustConfig>,
    pub interface: Option<InterfaceConfig>,
    pub tinygo: Option<TinyGoConfig>,
}

#[derive(serde::Deserialize, Debug, PartialEq)]
pub struct TinyGoConfig {}

pub fn get_config(path: Option<PathBuf>) -> Result<ProjectConfig> {
    let path_to_file = path
        .unwrap_or_else(|| PathBuf::from("."))
        .join("wasmcloud.toml");

    let config = Config::builder()
        .add_source(config::File::from(path_to_file))
        .add_source(config::Environment::with_prefix("WASMCLOUD"))
        .build()?;

    let value = config.try_deserialize::<serde_json::Value>()?;

    let raw_project_config: RawProjectConfig = serde_json::from_value(value)?;

    raw_project_config.try_into()
}

impl TryFrom<RawProjectConfig> for ProjectConfig {
    type Error = anyhow::Error;

    fn try_from(raw_project_config: RawProjectConfig) -> Result<Self> {
        let project_type_config = match raw_project_config.project_type.as_str() {
            "actor" => {
                let actor_config = raw_project_config
                    .actor
                    .ok_or_else(|| anyhow!("Missing actor config in wasmcloud.toml"))?;
                TypeConfig::Actor(actor_config)
            }

            "provider" => {
                let provider_config = raw_project_config
                    .provider
                    .ok_or_else(|| anyhow!("Missing provider config in wasmcloud.toml"))?;
                TypeConfig::Provider(provider_config)
            }

            "interface" => {
                let interface_config = raw_project_config
                    .interface
                    .ok_or_else(|| anyhow!("Missing interface config in wasmcloud.toml"))?;
                TypeConfig::Interface(interface_config)
            }

            _ => {
                return Err(anyhow!(
                    "Unknown project type: {}",
                    raw_project_config.project_type
                ));
            }
        };

        let language_config = match raw_project_config.language.as_str() {
            "rust" => {
                let rust_config = raw_project_config
                    .rust
                    .ok_or_else(|| anyhow!("Missing rust config in wasmcloud.toml"))?;
                LanguageConfig::Rust(rust_config)
            }
            "tinygo" => {
                let tinygo_config = raw_project_config
                    .tinygo
                    .ok_or_else(|| anyhow!("Missing tinygo config in wasmcloud.toml"))?;
                LanguageConfig::TinyGo(tinygo_config)
            }
            _ => {
                return Err(anyhow!(
                    "Unknown language in wasmcloud.toml: {}",
                    raw_project_config.language
                ));
            }
        };

        Ok(Self {
            language: language_config,
            project_type: project_type_config,
            name: raw_project_config.name,
            version: raw_project_config.version,
        })
    }
}
