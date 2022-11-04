use anyhow::Result;
use clap::Parser;
use serde_json::json;
use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

use crate::appearance::spinner::Spinner;
use crate::cfg::cfg_dir;
use crate::up::DOWNLOADS_DIR;
use crate::util::{CommandOutput, OutputKind};
use wash_lib::start::*;

#[derive(Parser, Debug, Clone)]
pub(crate) struct DownCommand {}

pub(crate) async fn handle_command(
    command: DownCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    handle_down(command, output_kind).await
}

pub(crate) async fn handle_down(
    _cmd: DownCommand,
    output_kind: OutputKind,
) -> Result<CommandOutput> {
    let install_dir = cfg_dir()?.join(DOWNLOADS_DIR);
    let sp = Spinner::new(&output_kind)?;

    let mut out_json = HashMap::new();
    let mut out_text = String::from("");
    let host_cmd = install_dir.join(WASMCLOUD_HOST_BIN);
    if host_cmd.is_file() {
        sp.update_spinner_message(" Stopping host ...".to_string());
        let output = Command::new(&host_cmd)
            .arg("stop")
            .stdin(Stdio::null())
            .output()
            .await;
        if let Ok(output) = output {
            if output.stderr.is_empty() && output.stdout.is_empty() {
                // if there was a host running, 'stop' has no output.
                // Give it time to stop before stopping nats
                tokio::time::sleep(Duration::from_secs(6)).await;
                out_json.insert("host_stopped".to_string(), json!(true));
                out_text.push_str("✅ wasmCloud host stopped successfully\n");
            } else {
                out_json.insert("host_stopped".to_string(), json!(true));
                out_text.push_str(
                    "🤔 Host did not appear to be running, assuming it's already stopped\n",
                );
            }
        }
    }

    let nats_cmd = install_dir.join(NATS_SERVER_BINARY);
    if nats_cmd.is_file() {
        let pid_file = install_dir.join(NATS_SERVER_PID);
        let signal = if pid_file.is_file() {
            format!("stop={}", &pid_file.display())
        } else {
            "stop".into()
        };
        sp.update_spinner_message(" Stopping NATS server ...".to_string());
        if let Err(e) = Command::new(nats_cmd)
            .arg("--signal")
            .arg(signal)
            .stdin(Stdio::null())
            .output()
            .await
        {
            out_json.insert("nats_stopped".to_string(), json!(false));
            out_text.push_str(&format!(
                "❌ NATS server did not stop successfully: {:?}\n",
                e
            ));
        } else {
            out_json.insert("nats_stopped".to_string(), json!(true));
            out_text.push_str("✅ NATS server stopped successfully\n");
        }
        // remove PID file
        if pid_file.is_file() {
            let _ = tokio::fs::remove_file(&pid_file).await;
        }
    }

    out_json.insert("success".to_string(), json!(true));
    out_text.push_str("🛁 wash down completed successfully");

    sp.finish_and_clear();
    Ok(CommandOutput::new(out_text, out_json))
}
