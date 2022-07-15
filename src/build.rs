use crate::util::CommandOutput;
use anyhow::Result;
use clap::Parser;
use wasmcloud_config_parser::{
    ActorConfig, CommonConfig, InterfaceConfig, LanguageConfig, ProviderConfig, TypeConfig,
};

/// Build (and sign) a wasmCloud actor, provider, or interface
#[derive(Debug, Parser, Clone)]
#[clap(name = "build")]
pub(crate) struct BuildCli {}

/// Build options shared amoung actors/providers/interfaces.
struct CommonBuildOptions {}

pub(crate) fn handle_command(command: BuildCli) -> Result<CommandOutput> {
    let config = wasmcloud_config_parser::get_config(None, Some(true))?;

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

fn build_actor(
    command: BuildCli,
    actor_config: ActorConfig,
    language_config: LanguageConfig,
    common_config: CommonConfig,
) -> Result<CommandOutput> {
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
