use std::{net::SocketAddr, sync::Arc};

use anyhow::Context as _;
use clap::Args;
use tokio::select;
use tracing::info;

use crate::cli::{CliCommand, CliContext, CommandOutput};

#[derive(Debug, Clone, Args)]
pub struct HostCommand {
    /// The host group label to assign to the host
    #[clap(long = "host-group", default_value = "default")]
    pub host_group: String,

    /// NATS URL for the host to connect to
    #[clap(long = "nats-url", default_value = "nats://localhost:4222")]
    pub nats_url: String,

    /// The host name to assign to the host
    #[clap(long = "host-name")]
    pub host_name: Option<String>,

    /// The address on which the HTTP server will listen
    #[clap(long = "http-addr")]
    pub http_addr: Option<SocketAddr>,
}

impl CliCommand for HostCommand {
    async fn handle(&self, _ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let nats_client = wash_runtime::washlet::connect_nats(self.nats_url.clone(), None)
            .await
            .context("failed to connect to NATS")?;
        let nats_client = Arc::new(nats_client);

        let mut cluster_host_builder = wash_runtime::washlet::ClusterHostBuilder::default()
            .with_nats_client(nats_client.clone())
            .with_host_group(self.host_group.clone())
            .with_plugin(Arc::new(
                wash_runtime::washlet::plugins::wasi_config::RuntimeConfig::default(),
            ))?
            .with_plugin(Arc::new(
                wash_runtime::washlet::plugins::wasi_logging::TracingLogging::default(),
            ))?
            .with_plugin(Arc::new(
                wash_runtime::washlet::plugins::wasi_blobstore::WasiBlobstore::new(None),
            ))?
            .with_plugin(Arc::new(
                wash_runtime::washlet::plugins::wasmcloud_messaging::WasmcloudMessaging::new(
                    nats_client.clone(),
                ),
            ))?
            .with_plugin(Arc::new(
                wash_runtime::washlet::plugins::wasi_keyvalue::WasiKeyvalue::new(nats_client),
            ))?;

        if let Some(host_name) = &self.host_name {
            cluster_host_builder = cluster_host_builder.with_host_name(host_name);
        }

        if let Some(addr) = self.http_addr {
            tracing::info!(addr = ?addr, "Starting HTTP server for components");
            cluster_host_builder = cluster_host_builder.with_plugin(Arc::new(
                wash_runtime::washlet::plugins::wasi_http::HttpServer::new(addr),
            ))?;
        }
        let cluster_host = cluster_host_builder
            .build()
            .context("failed to build cluster host")?;
        let host_cleanup = wash_runtime::washlet::run_cluster_host(cluster_host)
            .await
            .context("failed to start cluster node")?;

        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;

        select! {
            _ = interrupt.recv() => {
                info!("Stopping host...");
            },
        }
        host_cleanup.await?;

        Ok(CommandOutput::ok(
            "Host exited successfully".to_string(),
            None,
        ))
    }
}
