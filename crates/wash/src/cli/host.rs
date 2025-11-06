use std::{net::SocketAddr, sync::Arc};

use anyhow::Context as _;
use clap::Args;
use tracing::info;

use crate::cli::{CliCommand, CliContext, CommandOutput};

#[derive(Debug, Clone, Args)]
pub struct HostCommand {
    /// The host group label to assign to the host
    #[clap(long = "host-group", default_value = "default")]
    pub host_group: String,

    /// NATS URL for Control Plane communications
    #[clap(long = "scheduler-nats-url", default_value = "nats://localhost:4222")]
    pub scheduler_nats_url: String,

    /// NATS URL for Data Plane communications
    #[clap(long = "data-nats-url", default_value = "nats://localhost:4222")]
    pub data_nats_url: String,

    /// The host name to assign to the host
    #[clap(long = "host-name")]
    pub host_name: Option<String>,

    /// The address on which the HTTP server will listen
    #[clap(long = "http-addr")]
    pub http_addr: Option<SocketAddr>,
}

impl CliCommand for HostCommand {
    async fn handle(&self, _ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let scheduler_nats_client =
            wash_runtime::washlet::connect_nats(self.scheduler_nats_url.clone(), None)
                .await
                .context("failed to connect to NATS Scheduler URL")?;

        let data_nats_client =
            wash_runtime::washlet::connect_nats(self.data_nats_url.clone(), None)
                .await
                .context("failed to connect to NATS")?;
        let data_nats_client = Arc::new(data_nats_client);

        let mut cluster_host_builder = wash_runtime::washlet::ClusterHostBuilder::default()
            .with_nats_client(Arc::new(scheduler_nats_client))
            .with_host_group(self.host_group.clone())
            .with_plugin(Arc::new(
                wash_runtime::washlet::plugins::wasi_config::WasiConfig::default(),
            ))?
            .with_plugin(Arc::new(
                wash_runtime::washlet::plugins::wasi_logging::TracingLogging::default(),
            ))?
            .with_plugin(Arc::new(
                wash_runtime::washlet::plugins::wasi_blobstore::WasiBlobstore::new(None),
            ))?
            .with_plugin(Arc::new(
                wash_runtime::plugin::wasi_webgpu::WasiWebgpu::new(),
            ))?
            .with_plugin(Arc::new(
                wash_runtime::washlet::plugins::wasmcloud_messaging::WasmcloudMessaging::new(
                    data_nats_client.clone(),
                ),
            ))?
            .with_plugin(Arc::new(
                wash_runtime::washlet::plugins::wasi_keyvalue::WasiKeyvalue::new(
                    data_nats_client.clone(),
                ),
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

        tokio::signal::ctrl_c()
            .await
            .context("failed to listen for shutdown signal")?;

        info!("Stopping host...");

        host_cleanup.await?;

        Ok(CommandOutput::ok(
            "Host exited successfully".to_string(),
            None,
        ))
    }
}
