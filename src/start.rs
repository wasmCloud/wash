use crate::appearance::spinner::Spinner;
use crate::cfg::cfg_dir;
use crate::util::{CommandOutput, OutputKind};
use anyhow::{anyhow, Result};
use clap::Parser;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use wash_lib::start::*;

pub(crate) const SHOWER_EMOJI: &str = "\u{1F6BF}";
const DOWNLOADS_DIR: &str = "downloads";
const NATS_SERVER_VERSION: &str = "v2.8.4";
const WASMCLOUD_HOST_VERSION: &str = "v0.55.1";

#[derive(Parser, Debug, Clone)]
#[clap(name = "start")]
pub(crate) struct StartCommand {
    /// Launch NATS and wasmCloud connected to the current terminal, exiting on CTRL+c
    #[clap(long = "interactive")]
    pub(crate) interactive: bool,

    #[clap(flatten)]
    pub(crate) nats_opts: NatsOpts,

    #[clap(flatten)]
    pub(crate) wasmcloud_opts: WasmcloudOpts,
    // supply NATS connections, attempt to use ports for downloaded nats maybe
    // all wasmcloud host configuration values
}

#[derive(Parser, Debug, Clone)]
pub(crate) struct NatsOpts {
    /// If a connection can't be established, exit and don't start a NATS server
    #[clap(long = "connect-only")]
    pub(crate) connect_only: bool,

    /// Specify a NATS server version to download, e.g. `v2.7.2`. See https://github.com/nats-io/nats-server/releases/ for releases
    #[clap(long = "nats-version", default_value = NATS_SERVER_VERSION)]
    pub(crate) nats_version: String,
}

#[derive(Parser, Debug, Clone)]
pub(crate) struct WasmcloudOpts {
    /// Specify a wasmCloud host version to download, e.g. `v0.55.0`. See https://github.com/wasmCloud/wasmcloud-otp/releases for releases
    #[clap(long = "wasmcloud-version", default_value = WASMCLOUD_HOST_VERSION)]
    pub(crate) wasmcloud_version: String,
}

pub(crate) async fn handle_command(
    command: StartCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    handle_start(command, output_kind).await
}

pub(crate) async fn handle_start(
    cmd: StartCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let install_dir = cfg_dir()?.join(DOWNLOADS_DIR);
    let spinner = Spinner::new(&output_kind);

    // Avoid downloading + starting NATS if the user already runs their own server
    let mut nats_process = if !cmd.nats_opts.connect_only {
        // Download NATS if not already installed
        spinner.update_spinner_message(" Downloading NATS ...".to_string());
        let nats_binary = ensure_nats_server(&cmd.nats_opts.nats_version, &install_dir).await?;

        spinner.update_spinner_message(" Starting NATS ...".to_string());
        // Start NATS server, redirecting output to a log file
        let nats_log_path = install_dir.join("nats.log");
        let nats_log_file = tokio::fs::File::create(&nats_log_path)
            .await?
            .into_std()
            .await;
        Some(start_nats_server(nats_binary, nats_log_file, 4222).await?)
    } else {
        // If we can connect to NATS, return None as we aren't managing the child process.
        // Otherwise, exit with error since --connect-only was specified
        let server_address = format!("127.0.0.1:{}", 4222);
        tokio::net::TcpStream::connect(&server_address)
            .await
            .map(|_| None)
            .map_err(|_| {
                anyhow!(
                    "Could not connect to NATS at {}, exiting since --connect-only was set",
                    server_address
                )
            })?
    };

    // Download wasmCloud if not already installed
    spinner.update_spinner_message(" Downloading wasmCloud ...".to_string());
    let wasmcloud_executable =
        ensure_wasmcloud(&cmd.wasmcloud_opts.wasmcloud_version, &install_dir).await?;

    // Redirect output (which is on stderr) to a log file, or use the terminal for interactive mode
    spinner.update_spinner_message(" Starting wasmCloud ...".to_string());
    let stderr: std::process::Stdio = if cmd.interactive {
        std::process::Stdio::inherit()
    } else {
        let log_path = install_dir.join("wasmcloud.log");
        tokio::fs::File::create(&log_path)
            .await?
            .into_std()
            .await
            .into()
    };

    let mut wasmcloud_process = start_wasmcloud_host(
        wasmcloud_executable,
        std::process::Stdio::null(),
        stderr,
        std::collections::HashMap::new(),
    )
    .await?;

    spinner.finish_and_clear();
    if cmd.interactive {
        let running = Arc::new(AtomicBool::new(true));
        let r = running.clone();

        ctrlc::set_handler(move || {
            r.store(false, Ordering::SeqCst);
        })
        .expect("Error setting Ctrl-C handler");

        if output_kind != OutputKind::Json {
            println!(
                "🏃 Running in interactive mode, your host is running at http://localhost:4000",
            );
            println!("🚪 Press `CTRL+c` at any time to exit");
        }
        while running.load(Ordering::SeqCst) {}

        // Terminate wasmCloud and NATS processes
        wasmcloud_process.kill().await?;
        if let Some(mut process) = nats_process {
            process.kill().await?;
            // This avoids moving ownership so I can grab the pid later
            nats_process = Some(process)
        }
    }

    let mut out_json = HashMap::new();
    let mut out_text = String::from("");
    out_json.insert("success".to_string(), json!(true));
    out_text.push_str(&format!(
        "\n{} wash start completed successfully",
        SHOWER_EMOJI
    ));
    //TODO: capture pids, show command to kill them
    if let Some(Some(pid)) = nats_process.map(|child| child.id()) {
        out_json.insert("nats_pid".to_string(), json!(pid));
        out_text.push_str(&format!(
            "\n🕸  NATS is running in the background with a PID of {}, ",
            pid
        ));
    }
    if let Some(pid) = wasmcloud_process.id() {
        out_json.insert("wasmcloud_pid".to_string(), json!(pid));
        out_text.push_str(&format!(
            "\n🎛  wasmCloud is accessible at http://localhost:4000 and is running with a PID of {},",
            pid
        ));
    }

    Ok(CommandOutput::new(out_text, out_json))
}

// #[cfg(test)]
// mod tests {
//     use super::{PullCommand, PushCommand, RegCliCommand};
//     use clap::Parser;

//     const ECHO_WASM: &str = "wasmcloud.azurecr.io/echo:0.2.0";
//     const LOCAL_REGISTRY: &str = "localhost:5000";

//     #[derive(Debug, Parser)]
//     struct Cmd {
//         #[clap(subcommand)]
//         reg: RegCliCommand,
//     }

//     #[test]
//     /// Enumerates multiple options of the `pull` command to ensure API doesn't
//     /// change between versions. This test will fail if `wash reg pull`
//     /// changes syntax, ordering of required elements, or flags.
//     fn test_pull_comprehensive() {
//         // Not explicitly used, just a placeholder for a directory
//         const TESTDIR: &str = "./tests/fixtures";

//         let pull_basic: Cmd = Parser::try_parse_from(&["reg", "pull", ECHO_WASM]).unwrap();
//         let pull_all_flags: Cmd =
//             Parser::try_parse_from(&["reg", "pull", ECHO_WASM, "--allow-latest", "--insecure"])
//                 .unwrap();
//         let pull_all_options: Cmd = Parser::try_parse_from(&[
//             "reg",
//             "pull",
//             ECHO_WASM,
//             "--destination",
//             TESTDIR,
//             "--digest",
//             "sha256:a17a163afa8447622055deb049587641a9e23243a6cc4411eb33bd4267214cf3",
//             "--password",
//             "password",
//             "--user",
//             "user",
//         ])
//         .unwrap();
//         match pull_basic.reg {
//             RegCliCommand::Pull(PullCommand { url, .. }) => {
//                 assert_eq!(url, ECHO_WASM);
//             }
//             _ => panic!("`reg pull` constructed incorrect command"),
//         };

//         match pull_all_flags.reg {
//             RegCliCommand::Pull(PullCommand {
//                 url,
//                 allow_latest,
//                 opts,
//                 ..
//             }) => {
//                 assert_eq!(url, ECHO_WASM);
//                 assert!(allow_latest);
//                 assert!(opts.insecure);
//             }
//             _ => panic!("`reg pull` constructed incorrect command"),
//         };

//         match pull_all_options.reg {
//             RegCliCommand::Pull(PullCommand {
//                 url,
//                 destination,
//                 digest,
//                 opts,
//                 ..
//             }) => {
//                 assert_eq!(url, ECHO_WASM);
//                 assert_eq!(destination.unwrap(), TESTDIR);
//                 assert_eq!(
//                     digest.unwrap(),
//                     "sha256:a17a163afa8447622055deb049587641a9e23243a6cc4411eb33bd4267214cf3"
//                 );
//                 assert_eq!(opts.user.unwrap(), "user");
//                 assert_eq!(opts.password.unwrap(), "password");
//             }
//             _ => panic!("`reg pull` constructed incorrect command"),
//         };
//     }

//     #[test]
//     /// Enumerates multiple options of the `push` command to ensure API doesn't
//     /// change between versions. This test will fail if `wash reg push`
//     /// changes syntax, ordering of required elements, or flags.
//     fn test_push_comprehensive() {
//         // Not explicitly used, just a placeholder for a directory
//         const TESTDIR: &str = "./tests/fixtures";

//         // Push echo.wasm and pull from local registry
//         let echo_push_basic = &format!("{}/echo:pushbasic", LOCAL_REGISTRY);
//         let push_basic: Cmd = Parser::try_parse_from(&[
//             "reg",
//             "push",
//             echo_push_basic,
//             &format!("{}/echopush.wasm", TESTDIR),
//             "--insecure",
//         ])
//         .unwrap();
//         match push_basic.reg {
//             RegCliCommand::Push(PushCommand {
//                 url,
//                 artifact,
//                 opts,
//                 ..
//             }) => {
//                 assert_eq!(&url, echo_push_basic);
//                 assert_eq!(artifact, format!("{}/echopush.wasm", TESTDIR));
//                 assert!(opts.insecure);
//             }
//             _ => panic!("`reg push` constructed incorrect command"),
//         };

//         // Push logging.par.gz and pull from local registry
//         let logging_push_all_flags = &format!("{}/logging:allflags", LOCAL_REGISTRY);
//         let push_all_flags: Cmd = Parser::try_parse_from(&[
//             "reg",
//             "push",
//             logging_push_all_flags,
//             &format!("{}/logging.par.gz", TESTDIR),
//             "--insecure",
//             "--allow-latest",
//         ])
//         .unwrap();
//         match push_all_flags.reg {
//             RegCliCommand::Push(PushCommand {
//                 url,
//                 artifact,
//                 opts,
//                 allow_latest,
//                 ..
//             }) => {
//                 assert_eq!(&url, logging_push_all_flags);
//                 assert_eq!(artifact, format!("{}/logging.par.gz", TESTDIR));
//                 assert!(opts.insecure);
//                 assert!(allow_latest);
//             }
//             _ => panic!("`reg push` constructed incorrect command"),
//         };

//         // Push logging.par.gz to different tag and pull to confirm successful push
//         let logging_push_all_options = &format!("{}/logging:alloptions", LOCAL_REGISTRY);
//         let push_all_options: Cmd = Parser::try_parse_from(&[
//             "reg",
//             "push",
//             logging_push_all_options,
//             &format!("{}/logging.par.gz", TESTDIR),
//             "--allow-latest",
//             "--insecure",
//             "--config",
//             &format!("{}/config.json", TESTDIR),
//             "--password",
//             "supers3cr3t",
//             "--user",
//             "localuser",
//         ])
//         .unwrap();
//         match push_all_options.reg {
//             RegCliCommand::Push(PushCommand {
//                 url,
//                 artifact,
//                 opts,
//                 allow_latest,
//                 config,
//                 ..
//             }) => {
//                 assert_eq!(&url, logging_push_all_options);
//                 assert_eq!(artifact, format!("{}/logging.par.gz", TESTDIR));
//                 assert!(opts.insecure);
//                 assert!(allow_latest);
//                 assert_eq!(config.unwrap(), format!("{}/config.json", TESTDIR));
//                 assert_eq!(opts.user.unwrap(), "localuser");
//                 assert_eq!(opts.password.unwrap(), "supers3cr3t");
//             }
//             _ => panic!("`reg push` constructed incorrect command"),
//         };
//     }
// }
