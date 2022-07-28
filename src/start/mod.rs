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
mod config;
use config::*;

//TODO: enable tracing, do we pass environment down from the parent?

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
}

#[derive(Parser, Debug, Clone)]
pub(crate) struct NatsOpts {
    /// If a connection can't be established, exit and don't start a NATS server
    #[clap(long = "connect-only", env = "NATS_CONNECT_ONLY")]
    pub(crate) connect_only: bool,

    /// NATS server version to download, e.g. `v2.7.2`. See https://github.com/nats-io/nats-server/releases/ for releases
    #[clap(long = "nats-version", default_value = NATS_SERVER_VERSION, env = "NATS_VERSION")]
    pub(crate) nats_version: String,

    /// NATS server host to connect to
    #[clap(long = "nats-host", default_value = DEFAULT_NATS_HOST, env = "NATS_HOST")]
    pub(crate) nats_host: String,

    /// NATS server port to connect to. This will be used as the NATS listen port if `--connect-only` isn't set
    #[clap(long = "nats-port", default_value = DEFAULT_NATS_PORT, env = "NATS_PORT")]
    pub(crate) nats_port: u16,
}

#[derive(Parser, Debug, Clone)]
pub(crate) struct WasmcloudOpts {
    /// Path to a wash context with values to use to configure a host
    #[clap(long = "context")]
    pub(crate) context: Option<std::path::PathBuf>,

    /// wasmCloud host version to download, e.g. `v0.55.0`. See https://github.com/wasmCloud/wasmcloud-otp/releases for releases
    #[clap(long = "wasmcloud-version", default_value = WASMCLOUD_HOST_VERSION, env = "WASMCLOUD_VERSION")]
    pub(crate) wasmcloud_version: String,

    /// The prefix used to isolate multiple lattices from each other within the same NATS topic space (default: `default`)
    #[clap(
        short = 'x',
        long = "lattice-prefix",
        default_value = DEFAULT_LATTICE_PREFIX,
        env = WASMCLOUD_LATTICE_PREFIX,
    )]
    pub(crate) lattice_prefix: String,
    //TODO: come through and annotate these with descriptions
    #[clap(long = "host-seed", env = WASMCLOUD_HOST_SEED)]
    pub(crate) host_seed: Option<String>,

    #[clap(long = "rpc-host", default_value = DEFAULT_RPC_HOST, env = WASMCLOUD_RPC_HOST)]
    pub(crate) rpc_host: String,

    #[clap(long = "rpc-port", default_value = DEFAULT_RPC_PORT, env = WASMCLOUD_RPC_PORT)]
    pub(crate) rpc_port: String,

    #[clap(long = "rpc-seed", env = WASMCLOUD_RPC_SEED)]
    pub(crate) rpc_seed: Option<String>,

    #[clap(long = "rpc-timeout-ms", default_value = DEFAULT_RPC_TIMEOUT_MS, env = WASMCLOUD_RPC_TIMEOUT_MS)]
    pub(crate) rpc_timeout_ms: String,

    #[clap(long = "rpc-jwt", env = WASMCLOUD_RPC_JWT)]
    pub(crate) rpc_jwt: Option<String>,

    #[clap(long = "rpc-tls", env = WASMCLOUD_RPC_TLS)]
    pub(crate) rpc_tls: bool,

    #[clap(long = "prov-rpc-host", default_value = DEFAULT_PROV_RPC_HOST, env = WASMCLOUD_PROV_RPC_HOST)]
    pub(crate) prov_rpc_host: String,

    #[clap(long = "prov-rpc-port", default_value = DEFAULT_PROV_RPC_PORT, env = WASMCLOUD_PROV_RPC_PORT)]
    pub(crate) prov_rpc_port: String,

    #[clap(long = "prov-rpc-seed", env = WASMCLOUD_PROV_RPC_SEED)]
    pub(crate) prov_rpc_seed: Option<String>,

    #[clap(long = "prov-rpc-tls", env = WASMCLOUD_PROV_RPC_TLS)]
    pub(crate) prov_rpc_tls: bool,

    #[clap(long = "prov-rpc-jwt", env = WASMCLOUD_PROV_RPC_JWT)]
    pub(crate) prov_rpc_jwt: Option<String>,

    #[clap(long = "ctl-host", default_value = DEFAULT_CTL_HOST, env = WASMCLOUD_CTL_HOST)]
    pub(crate) ctl_host: String,

    #[clap(long = "ctl-port", default_value = DEFAULT_CTL_PORT, env = WASMCLOUD_CTL_PORT)]
    pub(crate) ctl_port: String,

    #[clap(long = "ctl-seed", env = WASMCLOUD_CTL_SEED)]
    pub(crate) ctl_seed: Option<String>,

    #[clap(long = "ctl-jwt", env = WASMCLOUD_CTL_JWT)]
    pub(crate) ctl_jwt: Option<String>,

    #[clap(long = "ctl-tls", env = WASMCLOUD_CTL_TLS)]
    pub(crate) ctl_tls: bool,

    #[clap(long = "cluster-seed", env = WASMCLOUD_CLUSTER_SEED)]
    pub(crate) cluster_seed: Option<String>,

    #[clap(long = "cluster-issuers", env = WASMCLOUD_CLUSTER_ISSUERS)]
    pub(crate) cluster_issuers: Option<Vec<String>>,

    #[clap(long = "provider-delay", default_value = DEFAULT_PROV_SHUTDOWN_DELAY_MS, env = WASMCLOUD_PROV_SHUTDOWN_DELAY_MS)]
    pub(crate) provider_delay: String,

    #[clap(long = "allow-latest", env = WASMCLOUD_OCI_ALLOW_LATEST)]
    pub(crate) allow_latest: bool,

    #[clap(long = "allowed-insecure", env = WASMCLOUD_OCI_ALLOWED_INSECURE)]
    pub(crate) allowed_insecure: Option<Vec<String>>,

    #[clap(long = "js-domain", env = WASMCLOUD_JS_DOMAIN)]
    pub(crate) js_domain: Option<String>,

    #[clap(long = "config-service-enabled", env = WASMCLOUD_CONFIG_SERVICE)]
    pub(crate) config_service_enabled: bool,

    #[clap(
        long = "enable-structured-logging",
        env = WASMCLOUD_STRUCTURED_LOGGING_ENABLED
    )]
    pub(crate) enable_structured_logging: bool,

    #[clap(long = "structured-log-level", default_value = DEFAULT_STRUCTURED_LOG_LEVEL, env = WASMCLOUD_STRUCTURED_LOG_LEVEL)]
    pub(crate) structured_log_level: String,

    #[clap(long = "enable-ipv6", env = WASMCLOUD_ENABLE_IPV6)]
    pub(crate) enable_ipv6: bool,
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
        Some(start_nats_server(nats_binary, nats_log_file, cmd.nats_opts.nats_port).await?)
    } else {
        // If we can connect to NATS, return None as we aren't managing the child process.
        // Otherwise, exit with error since --connect-only was specified
        let server_address = format!("{}:{}", cmd.nats_opts.nats_host, cmd.nats_opts.nats_port);
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

    let host_env = configure_host_env(cmd.wasmcloud_opts);
    let mut wasmcloud_process = start_wasmcloud_host(
        &wasmcloud_executable,
        std::process::Stdio::null(),
        stderr,
        host_env,
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

    // Build the CommandOutput providing some useful information like pids & ports
    let mut out_json = HashMap::new();
    let mut out_text = String::from("");
    out_json.insert("success".to_string(), json!(true));
    out_text.push_str("\n🚿 wash start completed successfully");

    // Tuple of (nats_pid, wasmcloud_pid)
    let mut pids = (0, 0);
    if let Some(Some(pid)) = nats_process.map(|child| child.id()) {
        out_json.insert("nats_pid".to_string(), json!(pid));
        let url = format!("127.0.0.1:{}", cmd.nats_opts.nats_port);
        out_json.insert("nats_url".to_string(), json!(url));
        out_text.push_str(&format!(
            "\n🕸  NATS is running in the background at http://{}",
            url
        ));
        pids.0 = pid;
    }
    if let Some(pid) = wasmcloud_process.id() {
        out_json.insert("wasmcloud_pid".to_string(), json!(pid));
        let url = "http://localhost:4000";
        out_json.insert("wasmcloud_url".to_string(), json!(url));
        out_text.push_str(&format!("\n🎛  wasmCloud is accessible at {}", url));
        pids.1 = pid;
    }

    //TODO: change command based on Windows if `kill` isn't an alias
    match pids {
        (0, 0) => (),
        // NATS without wasmCloud would've failed before now
        (_npid, 0) => (),
        (0, _wpid) => {
            let kill_cmd = format!("{} stop", wasmcloud_executable.to_string_lossy());
            out_json.insert("kill_cmd".to_string(), json!(kill_cmd));
            out_text.push_str(&format!(
                "\n\n🔪 To stop the wasmCloud host, run `{}`",
                kill_cmd
            ));
        }
        (npid, _wpid) => {
            let kill_cmd = format!(
                "{} stop; kill {}",
                wasmcloud_executable.to_string_lossy(),
                npid,
            );
            out_json.insert("kill_cmd".to_string(), json!(kill_cmd));
            out_text.push_str(&format!(
                "\n\n🔪 To stop the wasmCloud host and the NATS server, run `{}`",
                kill_cmd
            ));
        }
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
