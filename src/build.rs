use std::process;

use crate::util::CommandOutput;
use anyhow::Result;
use clap::Parser;
use wash_lib::parser::{
    ActorConfig, CommonConfig, InterfaceConfig, LanguageConfig, ProviderConfig, RustConfig,
    TinyGoConfig, TypeConfig,
};

/// Build (and sign) a wasmCloud actor, provider, or interface
#[derive(Debug, Parser, Clone)]
#[clap(name = "build")]
pub(crate) struct BuildCli {}

pub(crate) fn handle_command(command: BuildCli) -> Result<CommandOutput> {
    let config = wash_lib::parser::get_config(None, Some(true))?;

    match config.project_type {
        TypeConfig::Actor(actor_config) => {
            build_actor(command, actor_config, config.language, config.common)
        }
        TypeConfig::Provider(provider_config) => {
            build_provider(command, provider_config, config.language, config.common)
        }
        TypeConfig::Interface(interface_config) => {
            build_interface(command, interface_config, config.language, config.common)
        }
    }
}

fn build_rust(rust_config: RustConfig) -> Result<()> {
    let result = process::Command::new("cargo")
        .args(["build", "--release"])
        .output();

    match result {
        Ok(output) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn build_tinygo(tinygo_config: TinyGoConfig) -> Result<()> {
    todo!()
}

fn build_actor(
    command: BuildCli,
    actor_config: ActorConfig,
    language_config: LanguageConfig,
    common_config: CommonConfig,
) -> Result<CommandOutput> {
    // build it
    match language_config {
        LanguageConfig::Rust(rust_config) => {
            build_rust(rust_config)?;
        }
        LanguageConfig::TinyGo(tinygo_config) => {
            build_tinygo(tinygo_config)?;
        }
    }

    // sign it

    // push it

    // bop it

    todo!()
}

fn build_provider(
    command: BuildCli,
    provider_config: ProviderConfig,
    language_config: LanguageConfig,
    common_config: CommonConfig,
) -> Result<CommandOutput> {
    todo!()
}

fn build_interface(
    command: BuildCli,
    interface_config: InterfaceConfig,
    language_config: LanguageConfig,
    common_config: CommonConfig,
) -> Result<CommandOutput> {
    todo!()
}
