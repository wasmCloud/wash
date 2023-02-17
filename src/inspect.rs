use crate::util::{self};
use anyhow::{anyhow, bail, Result};
use clap::Parser;
use provider_archive::*;
use serde_json::json;
use std::{collections::HashMap, fs::File, io::Read, path::PathBuf};
use term_table::{row::Row, table_cell::*, Table};
use wascap::jwt::{Actor, Token};
use wash_lib::{
    cli::{cached_oci_file, claims::render_actor_claims, CommandOutput, OutputKind},
    registry::OciPullOptions,
};

const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6D];

#[derive(Debug, Parser, Clone)]
pub struct InspectCliCommand {
    /// Path to signed actor module or provider archive or OCI URL of signed actor module or provider archive
    pub module: String,

    /// Extract the raw JWT from the file and print to stdout
    #[clap(name = "jwt_only", long = "jwt-only")]
    jwt_only: bool,

    /// Digest to verify artifact against (if OCI URL is provided for <module> or <archive>)
    #[clap(short = 'd', long = "digest")]
    digest: Option<String>,

    /// Allow latest artifact tags (if OCI URL is provided for <module> or <archive>)
    #[clap(long = "allow-latest")]
    allow_latest: bool,

    /// OCI username, if omitted anonymous authentication will be used
    #[clap(
        short = 'u',
        long = "user",
        env = "WASH_REG_USER",
        hide_env_values = true
    )]
    user: Option<String>,

    /// OCI password, if omitted anonymous authentication will be used
    #[clap(
        short = 'p',
        long = "password",
        env = "WASH_REG_PASSWORD",
        hide_env_values = true
    )]
    password: Option<String>,

    /// Allow insecure (HTTP) registry connections
    #[clap(long = "insecure")]
    insecure: bool,

    /// skip the local OCI cache
    #[clap(long = "no-cache")]
    no_cache: bool,
}

/// Attempts to inspect a provider module or signed actor module
pub async fn handle_command(
    command: InspectCliCommand,
    _output_kind: OutputKind,
) -> Result<CommandOutput> {
    let mut buf = Vec::new();
    if PathBuf::from(command.module.clone()).as_path().is_dir() {
        let mut f = File::open(command.module.clone())?;
        f.read_to_end(&mut buf)?;
    } else {
        let cache_file =
            (!command.no_cache.clone()).then(|| cached_oci_file(&command.module.clone()));
        buf = wash_lib::registry::get_oci_artifact(
            command.module.clone(),
            cache_file,
            OciPullOptions {
                digest: command.digest.clone(),
                allow_latest: command.allow_latest.clone(),
                user: command.user.clone(),
                password: command.password.clone(),
                insecure: command.insecure.clone(),
            },
        )
        .await?;
    }

    if is_wasm(&buf) {
        handle_actor_module(command).await
    } else {
        handle_provider_archive(command).await
    }
}

/// Checks if the given file is a wasm file by searching a WASM magic number at the start of a binary
fn is_wasm(input: &[u8]) -> bool {
    if input.len() < 4 {
        return false;
    }
    return input[0..4] == WASM_MAGIC;
}

/// Extracts claims for a given OCI artifact
async fn get_caps(cmd: InspectCliCommand) -> Result<Option<Token<Actor>>> {
    let cache_path = (!cmd.no_cache).then(|| cached_oci_file(&cmd.module));
    let artifact_bytes = wash_lib::registry::get_oci_artifact(
        cmd.module,
        cache_path,
        OciPullOptions {
            digest: cmd.digest,
            allow_latest: cmd.allow_latest,
            user: cmd.user,
            password: cmd.password,
            insecure: cmd.insecure,
        },
    )
    .await?;
    // Extract will return an error if it encounters an invalid hash in the claims
    Ok(wascap::wasm::extract_claims(artifact_bytes)?)
}

/// Inspects an actor module
async fn handle_actor_module(cmd: InspectCliCommand) -> Result<CommandOutput> {
    let module_name = cmd.module.clone();
    let jwt_only = cmd.jwt_only;
    let caps = get_caps(cmd).await?;
    let out = match caps {
        Some(token) => {
            if jwt_only {
                CommandOutput::from_key_and_text("token", token.jwt)
            } else {
                let validation = wascap::jwt::validate_token::<Actor>(&token.jwt)?;
                render_actor_claims(token.claims, validation)
            }
        }
        None => bail!("No capabilities discovered in : {}", module_name),
    };
    Ok(out)
}

/// Inspects a provider archive
pub(crate) async fn handle_provider_archive(cmd: InspectCliCommand) -> Result<CommandOutput> {
    let cache_file = (!cmd.no_cache).then(|| cached_oci_file(&cmd.module));
    let artifact_bytes = wash_lib::registry::get_oci_artifact(
        cmd.module,
        cache_file,
        OciPullOptions {
            digest: cmd.digest,
            allow_latest: cmd.allow_latest,
            user: cmd.user,
            password: cmd.password,
            insecure: cmd.insecure,
        },
    )
    .await?;
    let artifact = ProviderArchive::try_load(&artifact_bytes)
        .await
        .map_err(|e| anyhow!("{}", e))?;
    let claims = artifact
        .claims()
        .ok_or_else(|| anyhow!("No claims found in artifact"))?;
    let metadata = claims
        .metadata
        .ok_or_else(|| anyhow!("No metadata found"))?;

    let friendly_rev = metadata
        .rev
        .map_or("None".to_string(), |rev| rev.to_string());
    let friendly_ver = metadata.ver.unwrap_or_else(|| "None".to_string());
    let name = metadata.name.unwrap_or_else(|| "None".to_string());
    let mut map = HashMap::new();
    map.insert("name".to_string(), json!(name));
    map.insert("issuer".to_string(), json!(claims.issuer));
    map.insert("service".to_string(), json!(claims.subject));
    map.insert("capability_contract_id".to_string(), json!(metadata.capid));
    map.insert("vendor".to_string(), json!(metadata.vendor));
    map.insert("version".to_string(), json!(friendly_ver));
    map.insert("revision".to_string(), json!(friendly_rev));
    map.insert("targets".to_string(), json!(artifact.targets()));
    let text_table = {
        let mut table = Table::new();
        util::configure_table_style(&mut table);

        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            format!("{} - Provider Archive", name),
            2,
            Alignment::Center,
        )]));

        table.add_row(Row::new(vec![
            TableCell::new("Account"),
            TableCell::new_with_alignment(claims.issuer, 1, Alignment::Right),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new("Service"),
            TableCell::new_with_alignment(claims.subject, 1, Alignment::Right),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new("Capability Contract ID"),
            TableCell::new_with_alignment(metadata.capid, 1, Alignment::Right),
        ]));
        table.add_row(Row::new(vec![
            TableCell::new("Vendor"),
            TableCell::new_with_alignment(metadata.vendor, 1, Alignment::Right),
        ]));

        table.add_row(Row::new(vec![
            TableCell::new("Version"),
            TableCell::new_with_alignment(friendly_ver, 1, Alignment::Right),
        ]));

        table.add_row(Row::new(vec![
            TableCell::new("Revision"),
            TableCell::new_with_alignment(friendly_rev, 1, Alignment::Right),
        ]));

        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            "Supported Architecture Targets",
            2,
            Alignment::Center,
        )]));

        table.add_row(Row::new(vec![TableCell::new_with_alignment(
            artifact.targets().join("\n"),
            2,
            Alignment::Left,
        )]));

        table.render()
    };

    Ok(CommandOutput::new(text_table, map))
}

#[cfg(test)]
mod test {
    use super::*;
    use clap::Parser;

    #[derive(Parser, Debug)]
    struct Cmd {
        #[clap(flatten)]
        command: InspectCliCommand,
    }

    #[test]
    /// Check all flags and options of the 'inspect' command
    /// so that the API does not change in between versions
    fn test_inspect_comprehensive() {
        const LOCAL: &str = "./coolthing.par.gz";
        const REMOTE: &str = "wasmcloud.azurecr.io/coolthing.par.gz";
        const SUBSCRIBER_OCI: &str = "wasmcloud.azurecr.io/subscriber:0.2.0";

        let inspect_long: Cmd = Parser::try_parse_from([
            "inspect",
            LOCAL,
            "--digest",
            "sha256:blah",
            "--password",
            "secret",
            "--user",
            "name",
            "--jwt-only",
            "--no-cache",
        ])
        .unwrap();
        match inspect_long.command {
            InspectCliCommand {
                module,
                jwt_only,
                digest,
                allow_latest,
                user,
                password,
                insecure,
                no_cache,
            } => {
                assert_eq!(module, LOCAL);
                assert_eq!(digest.unwrap(), "sha256:blah");
                assert!(!allow_latest);
                assert!(!insecure);
                assert_eq!(user.unwrap(), "name");
                assert_eq!(password.unwrap(), "secret");
                assert!(jwt_only);
                assert!(no_cache);
            }
        }
        let inspect_short: Cmd = Parser::try_parse_from([
            "inspect",
            REMOTE,
            "-d",
            "sha256:blah",
            "-p",
            "secret",
            "-u",
            "name",
            "--allow-latest",
            "--insecure",
            "--jwt-only",
            "--no-cache",
        ])
        .unwrap();
        match inspect_short.command {
            InspectCliCommand {
                module,
                jwt_only,
                digest,
                allow_latest,
                user,
                password,
                insecure,
                no_cache,
            } => {
                assert_eq!(module, REMOTE);
                assert_eq!(digest.unwrap(), "sha256:blah");
                assert!(allow_latest);
                assert!(insecure);
                assert_eq!(user.unwrap(), "name");
                assert_eq!(password.unwrap(), "secret");
                assert!(jwt_only);
                assert!(no_cache);
            }
        }

        let cmd: Cmd = Parser::try_parse_from([
            "inspect",
            SUBSCRIBER_OCI,
            "--digest",
            "sha256:5790f650cff526fcbc1271107a05111a6647002098b74a9a5e2e26e3c0a116b8",
            "--user",
            "name",
            "--password",
            "opensesame",
            "--allow-latest",
            "--insecure",
            "--jwt-only",
            "--no-cache",
        ])
        .unwrap();

        match cmd.command {
            InspectCliCommand {
                module,
                jwt_only,
                digest,
                allow_latest,
                user,
                password,
                insecure,
                no_cache,
            } => {
                assert_eq!(module, SUBSCRIBER_OCI);
                assert_eq!(
                    digest.unwrap(),
                    "sha256:5790f650cff526fcbc1271107a05111a6647002098b74a9a5e2e26e3c0a116b8"
                );
                assert_eq!(user.unwrap(), "name");
                assert_eq!(password.unwrap(), "opensesame");
                assert!(allow_latest);
                assert!(insecure);
                assert!(jwt_only);
                assert!(no_cache);
            }
        }

        let short_cmd: Cmd = Parser::try_parse_from([
            "inspect",
            SUBSCRIBER_OCI,
            "-d",
            "sha256:5790f650cff526fcbc1271107a05111a6647002098b74a9a5e2e26e3c0a116b8",
            "-u",
            "name",
            "-p",
            "opensesame",
            "--allow-latest",
            "--insecure",
            "--jwt-only",
            "--no-cache",
        ])
        .unwrap();

        match short_cmd.command {
            InspectCliCommand {
                module,
                jwt_only,
                digest,
                allow_latest,
                user,
                password,
                insecure,
                no_cache,
            } => {
                assert_eq!(module, SUBSCRIBER_OCI);
                assert_eq!(
                    digest.unwrap(),
                    "sha256:5790f650cff526fcbc1271107a05111a6647002098b74a9a5e2e26e3c0a116b8"
                );
                assert_eq!(user.unwrap(), "name");
                assert_eq!(password.unwrap(), "opensesame");
                assert!(allow_latest);
                assert!(insecure);
                assert!(jwt_only);
                assert!(no_cache);
            }
        }
    }
}
