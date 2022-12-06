use std::collections::HashMap;

use anyhow::Result;
use clap::Parser;
use serde_json::json;

use wash_lib::build::{build_actor, build_project};
use wash_lib::cli::CommandOutput;
use wash_lib::parser::{get_config, TypeConfig};

/// Build (and sign) a wasmCloud actor, provider, or interface
#[derive(Debug, Parser, Clone)]
#[clap(name = "build")]
pub(crate) struct BuildCommand {
    /// If set, pushes the signed actor to the registry.
    #[clap(short = 'p', long = "push")]
    pub(crate) push: bool,

    /// If set, skips signing the actor. Cannot be used with --push, as an actor has to be signed to push it to the registry.
    #[clap(long = "no-sign", conflicts_with = "push")]
    pub(crate) no_sign: bool,
}

pub(crate) fn handle_command(command: BuildCommand) -> Result<CommandOutput> {
    let config = get_config(None, Some(true))?;

    match config.project_type {
        TypeConfig::Actor(actor_config) => {
            let actor_path = build_actor(
                actor_config,
                config.language,
                config.common,
                command.no_sign,
            )?;
            let json_output = HashMap::from([
                ("actor_path".to_string(), json!(actor_path)),
                ("signed".to_string(), json!(!command.no_sign)),
            ]);
            Ok(CommandOutput::new(
                format!(
                    "Actor built and signed and can be found at {:?}",
                    actor_path
                ),
                json_output,
            ))
        }
        _ => {
            let path = build_project(config)?;
            // Until providers and interfaces have build support, this codepath won't be exercised
            Ok(CommandOutput::new(
                format!("Built artifact can be found at {:?}", path),
                HashMap::from([("path".to_string(), json!(path))]),
            ))
        }
    }
}

#[cfg(test)]
mod test {

    use super::*;
    use clap::Parser;

    #[test]
    fn test_build_comprehensive() {
        let cmd: BuildCommand = Parser::try_parse_from(["build", "--push"]).unwrap();
        assert!(cmd.push);

        let cmd: BuildCommand = Parser::try_parse_from(["build", "--no-sign"]).unwrap();
        assert!(cmd.no_sign);

        let cmd: BuildCommand = Parser::try_parse_from(["build"]).unwrap();
        assert!(!cmd.push);
        assert!(!cmd.no_sign);
    }
}
