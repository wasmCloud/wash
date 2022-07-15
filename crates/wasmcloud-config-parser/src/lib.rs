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
    /// The language of the project, e.g. rust, tinygo. Contains specific configuration for that language.
    pub language: LanguageConfig,
    /// The type of project, e.g. actor, provider, interface. Contains the specific configuration for that type.
    /// This is renamed to "type" but is named project_type here to avoid clashing with the type keyword in Rust.
    #[serde(rename = "type")]
    pub project_type: TypeConfig,
    /// The name of the project.
    pub name: String,
    /// The semantic version of the project.
    pub version: Version,
}

#[derive(serde::Deserialize, Debug, PartialEq)]
pub struct ActorConfig {
    /// The list of provider claims that this actor requires. eg. ["wasmcloud:httpserver"]
    pub claims: Option<Vec<String>>,
    /// The registry to push to. eg. "localhost:8080"
    pub registry: Option<String>,
    /// Whether to push to the registry insecurely. Defaults to false.
    pub push_insecure: bool,
    /// The directory to store the private keys in. Defaults to "./keys".
    pub key_directory: Option<PathBuf>,
    /// The filename of the signed wasm actor.
    pub filename: Option<String>,
    /// The target wasm target to build for. Defaults to "wasm32-unknown-unknown".
    pub wasm_target: Option<String>,
    /// The call alias of the actor. Defaults to no alias.
    pub call_alias: Option<String>,
}

#[derive(serde::Deserialize, Debug, PartialEq)]
pub struct ProviderConfig {
    /// The capability ID of the provider.
    pub capability_id: String,
    /// The vendor name of the provider. Optional, defaults to 'NoVendor'.
    pub vendor: Option<String>,
}

#[derive(serde::Deserialize, Debug, PartialEq)]
pub struct InterfaceConfig {}

#[derive(serde::Deserialize, Debug, PartialEq)]
pub struct RustConfig {
    /// The path to the cargo binary. Optional, will default tothe default `cargo` if not specified.
    pub cargo_path: Option<PathBuf>,
    /// Path to cargo/rust's `target` directory. Optional, defaults to `./target`.
    pub target_path: Option<PathBuf>,
}

#[derive(serde::Deserialize, Debug)]
struct RawProjectConfig {
    /// The language of the project, e.g. rust, tinygo. This is used to determine which config to parse.
    pub language: String,
    /// The type of project. This is a string that is used to determine which type of config to parse.
    /// The toml file name is just "type" but is named project_type here to avoid clashing with the type keyword in Rust.
    #[serde(rename = "type")]
    pub project_type: String,
    /// Name of the project.
    pub name: String,
    /// Semantic version of the project.
    pub version: Version,
    pub actor: Option<ActorConfig>,
    pub provider: Option<ProviderConfig>,
    pub rust: Option<RustConfig>,
    pub interface: Option<InterfaceConfig>,
    pub tinygo: Option<TinyGoConfig>,
}

#[derive(serde::Deserialize, Debug, PartialEq)]
pub struct TinyGoConfig {
    /// The path to the tinygo binary. Optional, will default to the default`tinygo` if not specified.
    pub tinygo_path: Option<PathBuf>,
}

/// Gets the wasmCloud project (actor, provider, or interface) config.
///
/// The config can come from multiple sources: a specific toml file path, a folder with a `wasmcloud.toml` file inside it, or by default it looks for a `wasmcloud.toml` file in the current directory.
///
/// The user can also override the config file by setting environment variables with the prefix "WASMCLOUD_". This behavior can be disabled by setting `use_env` to false.
/// For example, a user could set the variable `WASMCLOUD_RUST_CARGO_PATH` to override the default `cargo` path.
///
/// # Arguments
/// * `opt_path` - The path to the config file. If None, it will look for a wasmcloud.toml file in the current directory.
/// * `use_env` - Whether to use the environment variables or not. If false, it will not attempt to use environment variables. Defaults to true.
pub fn get_config(opt_path: Option<PathBuf>, use_env: Option<bool>) -> Result<ProjectConfig> {
    let mut path = opt_path.unwrap_or_else(|| PathBuf::from("."));

    if !path.exists() {
        return Err(anyhow!("Path {} does not exist", path.display()));
    }

    if path.is_dir() {
        let wasmcloud_path = path.join("wasmcloud.toml");
        if !wasmcloud_path.is_file() {
            return Err(anyhow!(
                "No wasmcloud.toml file found in {}",
                path.display()
            ));
        }
        path = wasmcloud_path;
    }

    if !path.is_file() {
        return Err(anyhow!("No config file found at {}", path.display()));
    }

    let mut config = Config::builder().add_source(config::File::from(path.clone()));

    if use_env.unwrap_or(true) {
        config = config.add_source(config::Environment::with_prefix("WASMCLOUD"));
    }

    let json_value = config
        .build()
        .map_err(|e| {
            if e.to_string().contains("is not of a registered file format") {
                return anyhow!("Invalid config file: {}", path.display());
            }

            anyhow!("{}", e)
        })?
        .try_deserialize::<serde_json::Value>()?;

    let raw_project_config: RawProjectConfig = serde_json::from_value(json_value)?;

    raw_project_config
        .try_into()
        .map_err(|e: anyhow::Error| anyhow!("{} in {}", e, path.display()))
}

impl TryFrom<RawProjectConfig> for ProjectConfig {
    type Error = anyhow::Error;

    fn try_from(raw_project_config: RawProjectConfig) -> Result<Self> {
        let project_type_config = match raw_project_config.project_type.as_str() {
            "actor" => {
                let actor_config = raw_project_config
                    .actor
                    .ok_or_else(|| anyhow!("Missing actor config"))?;
                TypeConfig::Actor(actor_config)
            }

            "provider" => {
                let provider_config = raw_project_config
                    .provider
                    .ok_or_else(|| anyhow!("Missing provider config"))?;
                TypeConfig::Provider(provider_config)
            }

            "interface" => {
                let interface_config = raw_project_config
                    .interface
                    .ok_or_else(|| anyhow!("Missing interface config"))?;
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
