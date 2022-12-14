use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use serde_json::json;

use wash_lib::build::{build_project, SignConfig};
use wash_lib::cli::CommandOutput;
use wash_lib::parser::{get_config, TypeConfig};

/// Build (and sign) a wasmCloud actor, provider, or interface
#[derive(Debug, Parser, Clone)]
#[clap(name = "build")]
pub(crate) struct BuildCommand {
    /// Path to the wasmcloud.toml file or parent folder to use for building
    #[clap(short = 'p', long = "config-path")]
    config_path: Option<PathBuf>,
    //TODO(brooksmtownsend): In the future, when we support building capability providers
    //for build, this will need to merge with the provider create options for a seamless build
    #[clap(flatten)]
    signing_config: SignConfig,
}

pub(crate) fn handle_command(command: BuildCommand) -> Result<CommandOutput> {
    let config = get_config(command.config_path, Some(true))?;

    match config.project_type {
        TypeConfig::Actor(ref _actor_config) => {
            let actor_path = build_project(&config, Some(command.signing_config))?;
            let json_output = HashMap::from([
                ("actor_path".to_string(), json!(actor_path)),
                ("signed".to_string(), json!(true)),
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
            // Until providers and interfaces have build support, this codepath won't be exercised
            let path = build_project(&config, None)?;
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
        let cmd: BuildCommand = Parser::try_parse_from(["build"]).unwrap();
        assert!(cmd.config_path.is_none());
        assert!(!cmd.signing_config.disable_keygen);
        assert!(cmd.signing_config.issuer.is_none());
        assert!(cmd.signing_config.subject.is_none());
        assert!(cmd.signing_config.keys_directory.is_none());

        let cmd: BuildCommand = Parser::try_parse_from([
            "build",
            "-p",
            "/",
            "--disable-keygen",
            "--issuer",
            "/tmp/iss.nk",
            "--subject",
            "/tmp/sub.nk",
            "--keys-directory",
            "/tmp",
        ])
        .unwrap();
        assert_eq!(cmd.config_path, Some(PathBuf::from("/")));
        assert!(cmd.signing_config.disable_keygen);
        assert_eq!(cmd.signing_config.issuer, Some("/tmp/iss.nk".to_string()));
        assert_eq!(cmd.signing_config.subject, Some("/tmp/sub.nk".to_string()));
        assert_eq!(
            cmd.signing_config.keys_directory,
            Some(PathBuf::from("/tmp"))
        );
    }
}
