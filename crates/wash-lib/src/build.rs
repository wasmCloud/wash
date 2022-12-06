//! Build (and sign) a wasmCloud actor, provider, or interface. Depends on the "cli" feature
//!

use std::{fs, path::PathBuf, process, str::FromStr};

use anyhow::{anyhow, bail, Result};

use crate::cli::{
    claims::{sign_file, ActorMetadata, SignCommand},
    OutputKind,
};
use crate::parser::{
    ActorConfig, CommonConfig, InterfaceConfig, LanguageConfig, ProjectConfig, ProviderConfig,
    RustConfig, TinyGoConfig, TypeConfig,
};

/// Using a [ProjectConfig], usually parsed from a `wasmcloud.toml` file, build the project
/// with the installed language toolchain. This will delegate to [build_actor] when the project is an actor,
/// or return an error when trying to build providers or interfaces. This functionality is planned in a future release.
///
/// This function returns the path to the compiled artifact, a signed Wasm module, signed provider archive, or compiled
/// interface library file.
///
/// # Usage
/// ```no_run
/// use wash_lib::{build::build_project, parser::get_config};
/// let config = get_config(None, Some(true))?;
/// let artifact_path = build_project(config)?;
/// println!("Here is the signed artifact: {}", artifact_path.to_string_lossy());
/// ```
pub fn build_project(config: ProjectConfig) -> Result<PathBuf> {
    match config.project_type {
        TypeConfig::Actor(actor_config) => {
            build_actor(actor_config, config.language, config.common, false)
        }
        TypeConfig::Provider(_provider_config) => Err(anyhow!(
            "wash build has not be implemented for providers yet. Please use `make` for now!"
        )),
        TypeConfig::Interface(_interface_config) => Err(anyhow!(
            "wash build has not be implemented for interfaces yet. Please use `make` for now!"
        )),
    }
}

/// Builds a wasmCloud actor using the installed language toolchain, then signs the actor with
/// keys, capability claims, and additional friendly information like name, version, revision, etc.
///
/// # Arguments
/// * `actor_config`: [ActorConfig] for required information to find, build, and sign an actor
/// * `language_config`: [LanguageConfig] specifying which language the actor is written in
/// * `common_config`: [CommonConfig] specifying common parameters like [CommonConfig::name] and [CommonConfig::version]
/// * `no_sign`: If `true`, build the actor but don't sign. Useful for just checking build status without needing signing keys or fully referenced capabilities
pub fn build_actor(
    actor_config: ActorConfig,
    language_config: LanguageConfig,
    common_config: CommonConfig,
    no_sign: bool,
) -> Result<PathBuf> {
    // Build actor based on language toolchain
    let file_path = match language_config {
        LanguageConfig::Rust(rust_config) => {
            build_rust_actor(common_config.clone(), rust_config, actor_config.clone())
        }
        LanguageConfig::TinyGo(tinygo_config) => {
            build_tinygo_actor(common_config.clone(), tinygo_config)
        }
    }?;

    // Exit early if signing isn't desired
    if no_sign {
        Ok(file_path)
    } else {
        let source = file_path
            .to_str()
            .ok_or_else(|| anyhow!("Could not convert file path to string"))?
            .to_string();

        let destination = format!("build/{}_s.wasm", common_config.name);
        let destination_file = PathBuf::from_str(&destination);

        let sign_options = SignCommand {
            source,
            destination: Some(destination),
            metadata: ActorMetadata {
                name: common_config.name,
                ver: Some(common_config.version.to_string()),
                custom_caps: actor_config.claims,
                call_alias: actor_config.call_alias,
                ..Default::default()
            },
        };
        sign_file(sign_options, OutputKind::Json)?;

        Ok(destination_file?)
    }
}

/// Builds a rust actor and returns the path to the file.
fn build_rust_actor(
    common_config: CommonConfig,
    rust_config: RustConfig,
    actor_config: ActorConfig,
) -> Result<PathBuf> {
    let mut command = match rust_config.cargo_path {
        Some(path) => process::Command::new(path),
        None => process::Command::new("cargo"),
    };

    let result = command.args(["build", "--release"]).status()?;

    if !result.success() {
        bail!("Compiling actor failed: {}", result.to_string())
    }

    let wasm_file = PathBuf::from(format!(
        "{}/{}/release/{}.wasm",
        rust_config
            .target_path
            .unwrap_or_else(|| PathBuf::from("target"))
            .to_string_lossy(),
        actor_config.wasm_target,
        common_config.name,
    ));

    if !wasm_file.exists() {
        bail!(
            "Could not find compiled wasm file to sign: {}",
            wasm_file.display()
        );
    }

    // move the file out into the build/ folder for parity with tinygo and convienience for users.
    let copied_wasm_file = PathBuf::from(format!("build/{}.wasm", common_config.name));
    if let Some(p) = copied_wasm_file.parent() {
        fs::create_dir_all(p)?;
    }
    fs::copy(&wasm_file, &copied_wasm_file)?;
    fs::remove_file(&wasm_file)?;

    Ok(copied_wasm_file)
}

/// Builds a tinygo actor and returns the path to the file.
fn build_tinygo_actor(common_config: CommonConfig, tinygo_config: TinyGoConfig) -> Result<PathBuf> {
    let filename = format!("build/{}.wasm", common_config.name);

    let mut command = match tinygo_config.tinygo_path {
        Some(path) => process::Command::new(path),
        None => process::Command::new("tinygo"),
    };

    if let Some(p) = PathBuf::from(&filename).parent() {
        fs::create_dir_all(p)?;
    }

    let result = command
        .args([
            "build",
            "-o",
            filename.as_str(),
            "-target",
            "wasm",
            "-scheduler",
            "none",
            "-no-debug",
            ".",
        ])
        .status()?;

    if !result.success() {
        bail!("Compiling actor failed: {}", result.to_string())
    }

    let wasm_file = PathBuf::from(filename);

    if !wasm_file.exists() {
        bail!(
            "Could not find compiled wasm file to sign: {}",
            wasm_file.display()
        );
    }

    Ok(wasm_file)
}

/// Placeholder for future functionality for building providers
#[allow(unused)]
fn build_provider(
    _provider_config: ProviderConfig,
    _language_config: LanguageConfig,
    _common_config: CommonConfig,
    _no_sign: bool,
) -> Result<()> {
    Ok(())
}

/// Placeholder for future functionality for building interfaces
#[allow(unused)]
fn build_interface(
    _interface_config: InterfaceConfig,
    _language_config: LanguageConfig,
    _common_config: CommonConfig,
) -> Result<()> {
    Ok(())
}
