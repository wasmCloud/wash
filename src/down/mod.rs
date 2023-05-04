use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::time::Duration;

use anyhow::{bail, Result};
use clap::Parser;
use serde_json::json;
use tokio::process::Command;
use wash_lib::cli::{CommandOutput, OutputKind};
use wash_lib::config::{DEFAULT_NATS_HOST, DEFAULT_NATS_PORT};
use wash_lib::start::{find_wasmcloud_binary, nats_pid_path, NATS_SERVER_BINARY};

use crate::appearance::spinner::Spinner;
use crate::cfg::cfg_dir;
use crate::up::{
    DEFAULT_LATTICE_PREFIX, DOWNLOADS_DIR, RELEASE_NODE, WASMCLOUD_CTL_CREDSFILE,
    WASMCLOUD_CTL_HOST, WASMCLOUD_CTL_JWT, WASMCLOUD_CTL_PORT, WASMCLOUD_CTL_SEED,
    WASMCLOUD_LATTICE_PREFIX, WASMCLOUD_PID_FILE,
};
use crate::util::{get_wasmcloud_hosts, nats_client_from_opts};

#[derive(Parser, Debug, Clone)]
pub(crate) struct DownCommand {
    /// A lattice prefix is a unique identifier for a lattice, and is frequently used within NATS topics to isolate messages from different lattices
    #[clap(
        short = 'x',
        long = "lattice-prefix",
        default_value = DEFAULT_LATTICE_PREFIX,
        env = WASMCLOUD_LATTICE_PREFIX,
    )]
    pub(crate) lattice_prefix: String,

    /// An IP address or DNS name to use to connect to NATS for Control Interface (CTL) messages, defaults to the value supplied to --nats-host if not supplied
    #[clap(long = "ctl-host", env = WASMCLOUD_CTL_HOST)]
    pub(crate) ctl_host: Option<String>,

    /// A port to use to connect to NATS for CTL messages, defaults to the value supplied to --nats-port if not supplied
    #[clap(long = "ctl-port", env = WASMCLOUD_CTL_PORT)]
    pub(crate) ctl_port: Option<u16>,

    /// Convenience flag for CTL authentication, internally this parses the JWT and seed from the credsfile
    #[clap(long = "ctl-credsfile", env = WASMCLOUD_CTL_CREDSFILE)]
    pub(crate) ctl_credsfile: Option<PathBuf>,

    /// A seed nkey to use to authenticate to NATS for CTL messages
    #[clap(long = "ctl-seed", env = WASMCLOUD_CTL_SEED, requires = "ctl_jwt")]
    pub(crate) ctl_seed: Option<String>,

    /// A user JWT to use to authenticate to NATS for CTL messages
    #[clap(long = "ctl-jwt", env = WASMCLOUD_CTL_JWT, requires = "ctl_seed")]
    pub(crate) ctl_jwt: Option<String>,

    #[clap(long = "host-id")]
    pub host_id: Option<String>,

    #[clap(long = "all")]
    pub all: bool,
}

pub(crate) async fn handle_command(
    command: DownCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    handle_down(command, output_kind).await
}

pub(crate) async fn handle_down(
    cmd: DownCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let install_dir = cfg_dir()?.join(DOWNLOADS_DIR);
    let sp = Spinner::new(&output_kind)?;

    let mut out_json = HashMap::new();
    let mut out_text = String::from("");
    let version = tokio::fs::read_to_string(install_dir.join(WASMCLOUD_PID_FILE))
        .await
        .map_err(|e| {
            anyhow::anyhow!("Unable to find wasmcloud pid file for stopping process: {e}")
        })?;
    let host_bin = find_wasmcloud_binary(&install_dir, &version)
        .await
        .ok_or_else(|| anyhow::anyhow!("Couldn't find path to wasmCloud binary. Is it running?"))?;

    let mut stopped_hosts = 0;
    let nats_client = nats_client_from_opts(
        &cmd.ctl_host.unwrap_or(DEFAULT_NATS_HOST.to_string()),
        &cmd.ctl_port
            .map(|port| port.to_string())
            .unwrap_or(DEFAULT_NATS_PORT.to_string()),
        cmd.ctl_jwt,
        cmd.ctl_seed,
        cmd.ctl_credsfile,
    )
    .await?;
    let hosts = get_wasmcloud_hosts(host_bin.clone(), nats_client).await?;

    if hosts.is_empty() {
        bail!("No local wasmCloud hosts detected.");
    } else if let Some(host_id) = cmd.host_id {
        let host = match hosts.iter().find(|h| h.id == host_id) {
            Some(host) => host,
            None => bail!("Host {} is not running", host_id),
        };
        sp.update_spinner_message(format!("Stopping host {} ...", host.id));
        (out_json, out_text, stopped_hosts) = stop_host(
            &host_bin,
            host.id.clone(),
            out_json,
            out_text,
            stopped_hosts,
        )
        .await;
    } else if hosts.len() == 1 {
        let host = hosts.first().unwrap();
        sp.update_spinner_message(format!("Stopping host {} ...", host.id));
        (out_json, out_text, stopped_hosts) = stop_host(
            &host_bin,
            host.id.clone(),
            out_json,
            out_text,
            stopped_hosts,
        )
        .await;
    } else if cmd.all {
        for (i, host) in hosts.iter().enumerate() {
            sp.update_spinner_message(format!(
                "Stopping host {} ({}/{})...",
                host.id,
                i + 1,
                hosts.len()
            ));
            (out_json, out_text, stopped_hosts) = stop_host(
                &host_bin,
                host.id.clone(),
                out_json,
                out_text,
                stopped_hosts,
            )
            .await;
        }
    } else {
        bail!(
            "Detected multiple wasmCloud hosts. Specify a host with --host-id [hostID], or use --all\nTo see your hosts, run wash ctl get hosts."
        )
    }

    // if there was a host running, 'stop' has no output.
    // Give it time to stop before stopping nats
    tokio::time::sleep(Duration::from_secs(6)).await;

    if stopped_hosts == hosts.len() {
        let nats_bin = install_dir.join(NATS_SERVER_BINARY);
        if nats_bin.is_file() {
            sp.update_spinner_message(" Stopping NATS server ...".to_string());
            if let Err(e) = stop_nats(install_dir).await {
                out_json.insert("nats_stopped".to_string(), json!(false));
                out_text.push_str(&format!(
                    "❌ NATS server did not stop successfully: {e:?}\n"
                ));
            } else {
                out_json.insert("nats_stopped".to_string(), json!(true));
                out_text.push_str("✅ NATS server stopped successfully\n");
            }
        }
    } else {
        out_json.insert("nats_stopped".to_string(), json!(false));
        out_text.push_str("NATS server is still running\n");
    }

    out_json.insert("success".to_string(), json!(true));
    out_text.push_str("🛁 wash down completed successfully");

    sp.finish_and_clear();
    Ok(CommandOutput::new(out_text, out_json))
}

pub(crate) async fn stop_host(
    host_bin: &PathBuf,
    host_id: String,
    mut out_json: HashMap<String, serde_json::Value>,
    mut out_text: String,
    mut stopped_hosts: usize,
) -> (HashMap<String, serde_json::Value>, String, usize) {
    if let Ok(output) = stop_wasmcloud(&host_bin, Some(host_id.to_string())).await {
        if output.stderr.is_empty() && output.stdout.is_empty() {
            stopped_hosts += 1;
            out_json.insert("host_stopped".to_string(), json!(host_id));
            out_text.push_str(&format!(
                "✅ wasmCloud host {} stopped successfully\n",
                host_id,
            ));
        } else {
            out_json.insert("host_stopped".to_string(), json!(host_id));
            out_text.push_str(&format!(
                "🤔 Host {} did not appear to be running, assuming it's already stopped\n",
                host_id
            ));
        }
    }
    (out_json, out_text, stopped_hosts)
}

/// Helper function to send wasmCloud the `stop` command and wait for it to clean up
pub(crate) async fn stop_wasmcloud<P>(bin_path: P, host_id: Option<String>) -> Result<Output>
where
    P: AsRef<Path>,
{
    Command::new(bin_path.as_ref())
        .stdout(Stdio::piped())
        .arg("stop")
        .env(RELEASE_NODE, host_id.unwrap_or("".to_string()))
        .output()
        .await
        .map_err(anyhow::Error::from)
}

/// Helper function to send the nats-server the stop command
pub(crate) async fn stop_nats<P>(install_dir: P) -> Result<Output>
where
    P: AsRef<Path>,
{
    let bin_path = install_dir.as_ref().join(NATS_SERVER_BINARY);
    let pid_file = nats_pid_path(install_dir);
    let signal = if pid_file.is_file() {
        format!("stop={}", &pid_file.display())
    } else {
        "stop".into()
    };
    let output = Command::new(bin_path)
        .arg("--signal")
        .arg(signal)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(anyhow::Error::from);

    // remove PID file
    if pid_file.is_file() {
        let _ = tokio::fs::remove_file(&pid_file).await;
    }
    output
}
