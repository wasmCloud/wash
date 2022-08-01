use crate::appearance::spinner::Spinner;
use crate::cfg::cfg_dir;
use crate::util::{CommandOutput, OutputKind};
use anyhow::{anyhow, Result};
use clap::Parser;
use serde_json::json;
use std::collections::HashMap;
use std::fmt::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};

use wash_lib::start::*;
mod config;
mod credsfile;
use config::*;

#[derive(Parser, Debug, Clone)]
pub(crate) struct UpCommand {
    /// Launch NATS and wasmCloud connected to the current terminal, exiting on CTRL+c
    #[clap(short = 'i', long = "interactive")]
    pub(crate) interactive: bool,

    #[clap(flatten)]
    pub(crate) nats_opts: NatsOpts,

    #[clap(flatten)]
    pub(crate) wasmcloud_opts: WasmcloudOpts,
}

#[derive(Parser, Debug, Clone)]
pub(crate) struct NatsOpts {
    /// If a connection can't be established, exit and don't start a NATS server. Will be ignored if a remote_url and credsfile are specified
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

    /// NATS Server Jetstream domain, defaults to `core`
    #[clap(long = "nats-js-domain", env = "NATS_JS_DOMAIN")]
    pub(crate) nats_js_domain: Option<String>,

    /// Optional remote URL of existing NATS infrastructure to extend. Must be provided with `nats-credentials`
    #[clap(long = "nats-remote-url", env = "NATS_REMOTE_URL")]
    pub(crate) nats_remote_url: Option<String>,

    /// Optional path to a NATS credentials file to authenticate and extend existing NATS infrastructure. Must be provided with `nats-remote-url`
    #[clap(long = "nats-credsfile", env = "NATS_CREDSFILE")]
    pub(crate) nats_credsfile: Option<PathBuf>,
}

impl From<NatsOpts> for NatsConfig {
    fn from(other: NatsOpts) -> NatsConfig {
        NatsConfig {
            host: other.nats_host,
            port: other.nats_port,
            js_domain: other.nats_js_domain,
            remote_url: other.nats_remote_url,
            credentials: other.nats_credsfile,
        }
    }
}

#[derive(Parser, Debug, Clone)]
pub(crate) struct WasmcloudOpts {
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

    ///
    #[clap(long = "host-seed", env = WASMCLOUD_HOST_SEED)]
    pub(crate) host_seed: Option<String>,

    // host, port, seed, timeout, jwt, tls, credsfile
    /// Defaults to --nats-host if not supplied
    #[clap(long = "rpc-host", env = WASMCLOUD_RPC_HOST)]
    pub(crate) rpc_host: Option<String>,

    ///
    /// Defaults to --nats-port if not supplied
    #[clap(long = "rpc-port", env = WASMCLOUD_RPC_PORT)]
    pub(crate) rpc_port: Option<u16>,

    ///
    #[clap(long = "rpc-seed", env = WASMCLOUD_RPC_SEED)]
    pub(crate) rpc_seed: Option<String>,

    ///
    #[clap(long = "rpc-timeout-ms", default_value = DEFAULT_RPC_TIMEOUT_MS, env = WASMCLOUD_RPC_TIMEOUT_MS)]
    pub(crate) rpc_timeout_ms: u32,

    ///
    #[clap(long = "rpc-jwt", env = WASMCLOUD_RPC_JWT)]
    pub(crate) rpc_jwt: Option<String>,

    ///
    #[clap(long = "rpc-tls", env = WASMCLOUD_RPC_TLS)]
    pub(crate) rpc_tls: bool,

    /// Convenience flag for RPC authentication, internally this parses the JWT and seed from the credsfile
    #[clap(long = "rpc-credsfile", env = WASMCLOUD_RPC_CREDSFILE)]
    pub(crate) rpc_credsfile: Option<PathBuf>,

    ///
    /// Defaults to --nats-host if not supplied
    #[clap(long = "prov-rpc-host", env = WASMCLOUD_PROV_RPC_HOST)]
    pub(crate) prov_rpc_host: Option<String>,

    ///
    /// Defaults to --nats-port if not supplied
    #[clap(long = "prov-rpc-port", env = WASMCLOUD_PROV_RPC_PORT)]
    pub(crate) prov_rpc_port: Option<u16>,

    ///
    #[clap(long = "prov-rpc-seed", env = WASMCLOUD_PROV_RPC_SEED)]
    pub(crate) prov_rpc_seed: Option<String>,

    ///
    #[clap(long = "prov-rpc-tls", env = WASMCLOUD_PROV_RPC_TLS)]
    pub(crate) prov_rpc_tls: bool,

    ///
    #[clap(long = "prov-rpc-jwt", env = WASMCLOUD_PROV_RPC_JWT)]
    pub(crate) prov_rpc_jwt: Option<String>,

    /// Convenience flag for Provider RPC authentication, internally this parses the JWT and seed from the credsfile
    #[clap(long = "prov-rpc-credsfile", env = WASMCLOUD_PROV_RPC_CREDSFILE)]
    pub(crate) prov_rpc_credsfile: Option<PathBuf>,

    ///
    /// Defaults to --nats-host if not supplied
    #[clap(long = "ctl-host", env = WASMCLOUD_CTL_HOST)]
    pub(crate) ctl_host: Option<String>,

    ///
    /// Defaults to --nats-port if not supplied
    #[clap(long = "ctl-port", env = WASMCLOUD_CTL_PORT)]
    pub(crate) ctl_port: Option<u16>,

    ///
    #[clap(long = "ctl-seed", env = WASMCLOUD_CTL_SEED)]
    pub(crate) ctl_seed: Option<String>,

    ///
    #[clap(long = "ctl-jwt", env = WASMCLOUD_CTL_JWT)]
    pub(crate) ctl_jwt: Option<String>,

    /// Convenience flag for CTL authentication, internally this parses the JWT and seed from the credsfile
    #[clap(long = "ctl-credsfile", env = WASMCLOUD_CTL_CREDSFILE)]
    pub(crate) ctl_credsfile: Option<PathBuf>,

    ///
    #[clap(long = "ctl-tls", env = WASMCLOUD_CTL_TLS)]
    pub(crate) ctl_tls: bool,

    ///
    #[clap(long = "cluster-seed", env = WASMCLOUD_CLUSTER_SEED)]
    pub(crate) cluster_seed: Option<String>,

    ///
    #[clap(long = "cluster-issuers", env = WASMCLOUD_CLUSTER_ISSUERS)]
    pub(crate) cluster_issuers: Option<Vec<String>>,

    ///
    #[clap(long = "provider-delay", default_value = DEFAULT_PROV_SHUTDOWN_DELAY_MS, env = WASMCLOUD_PROV_SHUTDOWN_DELAY_MS)]
    pub(crate) provider_delay: u32,

    ///
    #[clap(long = "allow-latest", env = WASMCLOUD_OCI_ALLOW_LATEST)]
    pub(crate) allow_latest: bool,

    ///
    #[clap(long = "allowed-insecure", env = WASMCLOUD_OCI_ALLOWED_INSECURE)]
    pub(crate) allowed_insecure: Option<Vec<String>>,

    /// Defaults to `core`
    #[clap(long = "wasmcloud-js-domain", env = WASMCLOUD_JS_DOMAIN)]
    pub(crate) wasmcloud_js_domain: Option<String>,

    ///
    #[clap(long = "config-service-enabled", env = WASMCLOUD_CONFIG_SERVICE)]
    pub(crate) config_service_enabled: bool,

    ///
    #[clap(
        long = "enable-structured-logging",
        env = WASMCLOUD_STRUCTURED_LOGGING_ENABLED
    )]
    pub(crate) enable_structured_logging: bool,

    ///
    #[clap(long = "structured-log-level", default_value = DEFAULT_STRUCTURED_LOG_LEVEL, env = WASMCLOUD_STRUCTURED_LOG_LEVEL)]
    pub(crate) structured_log_level: String,

    ///
    #[clap(long = "enable-ipv6", env = WASMCLOUD_ENABLE_IPV6)]
    pub(crate) enable_ipv6: bool,
}

pub(crate) async fn handle_command(
    command: UpCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    handle_up(command, output_kind).await
}

pub(crate) async fn handle_up(cmd: UpCommand, output_kind: OutputKind) -> Result<CommandOutput> {
    let install_dir = cfg_dir()?.join(DOWNLOADS_DIR);
    let spinner = Spinner::new(&output_kind);
    // Capture listen address to keep the value after the nats_opts are moved
    let nats_listen_address = format!("{}:{}", cmd.nats_opts.nats_host, cmd.nats_opts.nats_port);

    // Avoid downloading + starting NATS if the user already runs their own server. Ignore connect_only
    // if this server has a remote and credsfile as we have to start a leafnode in that scenario
    let nats_opts = cmd.nats_opts.clone();
    let mut nats_process = if !cmd.nats_opts.connect_only
        || cmd.nats_opts.nats_remote_url.is_some() && cmd.nats_opts.nats_credsfile.is_some()
    {
        if cmd.nats_opts.connect_only {
            log::warn!("--connect-only ignored as remote_url and credsfile are specified\n")
        };
        // Download NATS if not already installed
        spinner.update_spinner_message(" Downloading NATS ...".to_string());
        let nats_binary = ensure_nats_server(&cmd.nats_opts.nats_version, &install_dir).await?;

        spinner.update_spinner_message(" Starting NATS ...".to_string());
        // Ensure that leaf node remote connection can be established before launching NATS
        let nats_opts = match (
            cmd.nats_opts.nats_remote_url.as_ref(),
            cmd.nats_opts.nats_credsfile.as_ref(),
        ) {
            (Some(url), Some(creds)) => {
                if let Err(e) = crate::util::nats_client_from_opts(
                    url,
                    &cmd.nats_opts.nats_port.to_string(),
                    None,
                    None,
                    Some(creds.to_owned()),
                )
                .await
                {
                    return Err(anyhow!("Could not connect to leafnode remote: {}", e));
                } else {
                    cmd.nats_opts
                }
            }
            (_, _) => cmd.nats_opts,
        };
        // Start NATS server, redirecting output to a log file
        let nats_log_path = install_dir.join("nats.log");
        let nats_log_file = tokio::fs::File::create(&nats_log_path)
            .await?
            .into_std()
            .await;
        Some(start_nats_server(nats_binary, nats_log_file, nats_opts.into()).await?)
    } else {
        // If we can connect to NATS, return None as we aren't managing the child process.
        // Otherwise, exit with error since --connect-only was specified
        tokio::net::TcpStream::connect(&nats_listen_address)
            .await
            .map(|_| None)
            .map_err(|_| {
                anyhow!(
                    "Could not connect to NATS at {}, exiting since --connect-only was set",
                    nats_listen_address
                )
            })?
    };

    // Download wasmCloud if not already installed
    spinner.update_spinner_message(" Downloading wasmCloud ...".to_string());
    let wasmcloud_executable =
        ensure_wasmcloud(&cmd.wasmcloud_opts.wasmcloud_version, &install_dir).await?;

    // Redirect output (which is on stderr) to a log file, or use the terminal for interactive mode
    spinner.update_spinner_message(" Starting wasmCloud ...".to_string());
    let stderr: Stdio = if cmd.interactive {
        Stdio::piped()
    } else {
        let log_path = install_dir.join("wasmcloud.log");
        tokio::fs::File::create(&log_path)
            .await?
            .into_std()
            .await
            .into()
    };

    let host_env = configure_host_env(nats_opts, cmd.wasmcloud_opts).await;
    let mut wasmcloud_child = match start_wasmcloud_host(
        &wasmcloud_executable,
        std::process::Stdio::null(),
        stderr,
        host_env,
    )
    .await
    {
        Ok(child) => child,
        Err(e) => {
            // Ensure we clean up the NATS server if we can't start wasmCloud
            if let Some(mut process) = nats_process {
                process.kill().await?;
            }
            return Err(e);
        }
    };

    spinner.finish_and_clear();
    if cmd.interactive {
        let running = Arc::new(AtomicBool::new(true));
        let r = running.clone();

        ctrlc::set_handler(move || {
            if r.load(Ordering::SeqCst) {
                r.store(false, Ordering::SeqCst);
            } else {
                log::warn!("\nRepeated CTRL+C received, killing wasmCloud and NATS. This may result in zombie processes")
            }
        })
        .expect("Error setting Ctrl-C handler, please file a bug issue https://github.com/wasmCloud/wash/issues/new/choose");

        if output_kind != OutputKind::Json {
            println!(
                "🏃 Running in interactive mode, your host is running at http://localhost:4000",
            );
            println!("🚪 Press `CTRL+c` at any time to exit");
        }

        // Create a separate thread to log host output
        let handle = wasmcloud_child.stderr.take().map(|stderr| {
            tokio::spawn(async {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    //TODO: in the future, would be great print these in a prettier format
                    println!("{}", line)
                }
            })
        });

        while running.load(Ordering::SeqCst) {}
        // Prevent extraneous messages from the host getting printed as the host shuts down
        if let Some(handle) = handle {
            handle.abort()
        };
        let spinner = Spinner::new(&output_kind);
        spinner.update_spinner_message(
            "CTRL+c received, gracefully stopping wasmCloud and NATS...".to_string(),
        );

        // Terminate wasmCloud and NATS processes
        stop_wasmcloud(wasmcloud_executable.clone()).await?;

        if let Some(mut process) = nats_process {
            match process.try_wait() {
                Ok(Some(_)) => (),
                _ => process.kill().await?,
            }
            // This avoids moving ownership so I can grab the pid later
            nats_process = Some(process)
        }

        spinner.finish_and_clear();
    }

    // Build the CommandOutput providing some useful information like pids & ports
    let mut out_json = HashMap::new();
    let mut out_text = String::from("");
    out_json.insert("success".to_string(), json!(true));
    out_text.push_str("🛁 wash up completed successfully");

    let nats_pid = if let Some(Some(pid)) = nats_process.map(|child| child.id()) {
        out_json.insert("nats_pid".to_string(), json!(pid));
        out_json.insert("nats_url".to_string(), json!(nats_listen_address));
        let _ = write!(
            out_text,
            "\n🕸  NATS is running in the background at http://{}",
            nats_listen_address
        );
        Some(pid)
    } else {
        None
    };
    if !cmd.interactive {
        let url = "http://localhost:4000";
        out_json.insert("wasmcloud_url".to_string(), json!(url));

        let _ = write!(
            out_text,
            "\n🌐 The wasmCloud dashboard is running at {}",
            url
        );
    }

    if let Some(pid) = nats_pid {
        let kill_cmd = format!(
            "{} stop; kill {}",
            wasmcloud_executable.to_string_lossy(),
            pid,
        );
        out_json.insert("kill_cmd".to_string(), json!(kill_cmd));
        let _ = write!(
            out_text,
            "\n\n🛑 To stop the wasmCloud host and the NATS server, run:\n{}",
            kill_cmd
        );
    } else if !cmd.interactive {
        let kill_cmd = format!("{} stop", wasmcloud_executable.to_string_lossy());
        out_json.insert("kill_cmd".to_string(), json!(kill_cmd));
        let _ = write!(
            out_text,
            "\n\n🛑 To stop the wasmCloud host, run:\n{}",
            kill_cmd
        );
    }

    Ok(CommandOutput::new(out_text, out_json))
}

async fn stop_wasmcloud<P>(bin_path: P) -> Result<()>
where
    P: AsRef<Path>,
{
    let child = Command::new(bin_path.as_ref())
        .stdout(Stdio::piped())
        .arg("stop")
        .spawn()?;

    // Wait for the stop command to return "ok", then exit
    tokio::spawn(async {
        if let Some(stdout) = child.stdout {
            let mut lines = BufReader::new(stdout).lines();

            while let Ok(Some(line)) = lines.next_line().await {
                if line == "ok" {
                    return;
                }
            }
        }
    })
    .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::UpCommand;
    use anyhow::Result;
    use clap::Parser;

    // Assert that our API doesn't unknowingly drift
    #[test]
    fn test_up_comprehensive() -> Result<()> {
        // Not explicitly used, just a placeholder for a directory
        const TESTDIR: &str = "./tests/fixtures";

        let up_all_flags: UpCommand = Parser::try_parse_from(&[
            "up",
            "--allow-latest",
            "--allowed-insecure",
            "localhost:5000",
            "--cluster-issuers",
            "CBZZ6BLE7PIJNCEJMXOHAJ65KIXRVXDA74W6LUKXC4EPFHTJREXQCOYI",
            "--cluster-seed",
            "SCAKLQ2FFT4LZUUVQMH6N37US3IZUEVJBUR3V532VV3DAAHSZXPQY6DYIM",
            "--config-service-enabled",
            "--connect-only",
            "--ctl-credsfile",
            TESTDIR,
            "--ctl-host",
            "127.0.0.2",
            "--ctl-jwt",
            "eyyjWT",
            "--ctl-port",
            "4232",
            "--ctl-seed",
            "SUALIKDKMIUAKRT5536EXKC3CX73TJD3CFXZMJSHIKSP3LTYIIUQGCUVGA",
            "--ctl-tls",
            "--enable-ipv6",
            "--enable-structured-logging",
            "--host-seed",
            "SNAP4UVNHVWSBJ5MHAQ6M3RB23S3ALA3O3A4RF25G2FQB5CCZJBBBWCKBY",
            "--interactive",
            "--nats-credsfile",
            TESTDIR,
            "--nats-host",
            "127.0.0.2",
            "--nats-js-domain",
            "domain",
            "--nats-port",
            "4232",
            "--nats-remote-url",
            "tls://remote.global",
            "--nats-version",
            "v2.8.4",
            "--prov-rpc-credsfile",
            TESTDIR,
            "--prov-rpc-host",
            "127.0.0.2",
            "--prov-rpc-jwt",
            "eyyjWT",
            "--prov-rpc-port",
            "4232",
            "--prov-rpc-seed",
            "SUALIKDKMIUAKRT5536EXKC3CX73TJD3CFXZMJSHIKSP3LTYIIUQGCUVGA",
            "--prov-rpc-tls",
            "--provider-delay",
            "500",
            "--rpc-credsfile",
            TESTDIR,
            "--rpc-host",
            "127.0.0.2",
            "--rpc-jwt",
            "eyyjWT",
            "--rpc-port",
            "4232",
            "--rpc-seed",
            "SUALIKDKMIUAKRT5536EXKC3CX73TJD3CFXZMJSHIKSP3LTYIIUQGCUVGA",
            "--rpc-timeout-ms",
            "500",
            "--rpc-tls",
            "--structured-log-level",
            "warn",
            "--wasmcloud-js-domain",
            "domain",
            "--wasmcloud-version",
            "v0.55.1",
            "--lattice-prefix",
            "anotherprefix",
        ])?;
        assert!(up_all_flags.wasmcloud_opts.allow_latest);
        assert_eq!(
            up_all_flags.wasmcloud_opts.allowed_insecure,
            Some(vec!["localhost:5000".to_string()])
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.cluster_issuers,
            Some(vec![
                "CBZZ6BLE7PIJNCEJMXOHAJ65KIXRVXDA74W6LUKXC4EPFHTJREXQCOYI".to_string()
            ])
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.cluster_seed,
            Some("SCAKLQ2FFT4LZUUVQMH6N37US3IZUEVJBUR3V532VV3DAAHSZXPQY6DYIM".to_string())
        );
        assert!(up_all_flags.wasmcloud_opts.config_service_enabled);
        assert!(up_all_flags.nats_opts.connect_only);
        assert!(up_all_flags.wasmcloud_opts.ctl_credsfile.is_some());
        assert_eq!(
            up_all_flags.wasmcloud_opts.ctl_host,
            Some("127.0.0.2".to_string())
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.ctl_jwt,
            Some("eyyjWT".to_string())
        );
        assert_eq!(up_all_flags.wasmcloud_opts.ctl_port, Some(4232));
        assert_eq!(
            up_all_flags.wasmcloud_opts.ctl_seed,
            Some("SUALIKDKMIUAKRT5536EXKC3CX73TJD3CFXZMJSHIKSP3LTYIIUQGCUVGA".to_string())
        );
        assert!(up_all_flags.wasmcloud_opts.ctl_tls);
        assert!(up_all_flags.wasmcloud_opts.rpc_credsfile.is_some());
        assert_eq!(
            up_all_flags.wasmcloud_opts.rpc_host,
            Some("127.0.0.2".to_string())
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.rpc_jwt,
            Some("eyyjWT".to_string())
        );
        assert_eq!(up_all_flags.wasmcloud_opts.rpc_port, Some(4232));
        assert_eq!(
            up_all_flags.wasmcloud_opts.rpc_seed,
            Some("SUALIKDKMIUAKRT5536EXKC3CX73TJD3CFXZMJSHIKSP3LTYIIUQGCUVGA".to_string())
        );
        assert!(up_all_flags.wasmcloud_opts.rpc_tls);
        assert!(up_all_flags.wasmcloud_opts.prov_rpc_credsfile.is_some());
        assert_eq!(
            up_all_flags.wasmcloud_opts.prov_rpc_host,
            Some("127.0.0.2".to_string())
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.prov_rpc_jwt,
            Some("eyyjWT".to_string())
        );
        assert_eq!(up_all_flags.wasmcloud_opts.prov_rpc_port, Some(4232));
        assert_eq!(
            up_all_flags.wasmcloud_opts.prov_rpc_seed,
            Some("SUALIKDKMIUAKRT5536EXKC3CX73TJD3CFXZMJSHIKSP3LTYIIUQGCUVGA".to_string())
        );
        assert!(up_all_flags.wasmcloud_opts.prov_rpc_tls);
        assert!(up_all_flags.wasmcloud_opts.enable_ipv6);
        assert!(up_all_flags.wasmcloud_opts.enable_structured_logging);
        assert_eq!(
            up_all_flags.wasmcloud_opts.host_seed,
            Some("SNAP4UVNHVWSBJ5MHAQ6M3RB23S3ALA3O3A4RF25G2FQB5CCZJBBBWCKBY".to_string())
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.structured_log_level,
            "warn".to_string()
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.wasmcloud_version,
            "v0.55.1".to_string()
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.lattice_prefix,
            "anotherprefix".to_string()
        );
        assert_eq!(
            up_all_flags.wasmcloud_opts.wasmcloud_js_domain,
            Some("domain".to_string())
        );
        assert_eq!(up_all_flags.nats_opts.nats_version, "v2.8.4".to_string());
        assert_eq!(
            up_all_flags.nats_opts.nats_remote_url,
            Some("tls://remote.global".to_string())
        );
        assert_eq!(up_all_flags.wasmcloud_opts.provider_delay, 500);
        assert!(up_all_flags.interactive);

        Ok(())
    }
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
