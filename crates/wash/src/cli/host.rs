use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use anyhow::Context as _;
use clap::Args;
use tracing::info;
#[cfg(not(target_os = "windows"))]
use wash_runtime::plugin::wasi_webgpu::WasiWebGpu;

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

    /// Path to TLS CA certificate file for NATS connection
    #[clap(long = "nats-tls-ca")]
    pub nats_tls_ca: Option<PathBuf>,

    /// Enable TLS handshake first mode for NATS connection
    #[clap(long = "nats-tls-first", default_value_t = false)]
    pub nats_tls_first: bool,

    /// Path to NATS credentials file
    #[clap(long = "nats-creds")]
    pub nats_creds: Option<PathBuf>,

    /// The host name to assign to the host
    #[clap(long = "host-name")]
    pub host_name: Option<String>,

    /// The address on which the HTTP server will listen
    #[clap(long = "http-addr")]
    pub http_addr: Option<SocketAddr>,

    /// Enable WASI WebGPU support
    #[cfg(not(target_os = "windows"))]
    #[clap(long = "wasi-webgpu", default_value_t = false)]
    pub wasi_webgpu: bool,

    /// Allow insecure OCI Registries
    #[clap(long = "allow-insecure-registries", default_value_t = false)]
    pub allow_insecure_registries: bool,

    /// Timeout for pulling artifacts from OCI registries
    #[clap(long = "registry-pull-timeout", value_parser = humantime::parse_duration, default_value = "30s")]
    pub registry_pull_timeout: Duration,
}

impl CliCommand for HostCommand {
    async fn handle(&self, _ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let nats_options = Some(wash_runtime::washlet::NatsConnectionOptions {
            tls_ca: self.nats_tls_ca.clone(),
            tls_first: self.nats_tls_first,
            credentials: self.nats_creds.clone(),
            ..Default::default()
        });

        let scheduler_nats_client =
            wash_runtime::washlet::connect_nats(self.scheduler_nats_url.clone(), nats_options.clone())
                .await
                .context("failed to connect to NATS Scheduler URL")?;

        let data_nats_client =
            wash_runtime::washlet::connect_nats(self.data_nats_url.clone(), nats_options)
                .await
                .context("failed to connect to NATS")?;
        let data_nats_client = Arc::new(data_nats_client);

        let host_config = wash_runtime::host::HostConfig {
            allow_oci_insecure: self.allow_insecure_registries,
            oci_pull_timeout: Some(self.registry_pull_timeout),
        };

        let mut cluster_host_builder = wash_runtime::washlet::ClusterHostBuilder::default()
            .with_host_config(host_config)
            .with_nats_client(Arc::new(scheduler_nats_client))
            .with_host_group(self.host_group.clone())
            .with_plugin(Arc::new(
                wash_runtime::washlet::plugins::wasi_config::WasiConfig::default(),
            ))?
            .with_plugin(Arc::new(
                wash_runtime::washlet::plugins::wasi_logging::TracingLogging::default(),
            ))?
            .with_plugin(Arc::new(
                wash_runtime::washlet::plugins::wasi_blobstore::WasiBlobstore::new(
                    data_nats_client.clone(),
                ),
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
            let http_router = wash_runtime::host::http::DynamicRouter::default();
            cluster_host_builder = cluster_host_builder.with_http_handler(Arc::new(
                wash_runtime::host::http::HttpServer::new(http_router, addr),
            ));
        }

        // Enable WASI WebGPU if requested
        #[cfg(not(target_os = "windows"))]
        if self.wasi_webgpu {
            tracing::info!("WASI WebGPU support enabled");
            cluster_host_builder =
                cluster_host_builder.with_plugin(Arc::new(WasiWebGpu::default()))?;
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
