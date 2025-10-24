#![doc = include_str!("../README.md")]

pub mod engine;
pub mod host;
pub mod plugin;
pub mod types;
pub mod wit;

#[cfg(feature = "oci")]
pub mod oci;

// Re-export wasmtime for convenience
pub use wasmtime;

#[cfg(test)]
mod test {
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::plugin::wasi_config::RuntimeConfig;
    use crate::plugin::wasi_http::HttpServer;
    use crate::{
        host::HostApi,
        types::{Workload, WorkloadStartRequest},
    };

    use super::{engine::Engine, host::HostBuilder};

    #[tokio::test]
    #[cfg(feature = "wasi-http")]
    async fn can_run_engine() -> anyhow::Result<()> {
        let engine = Engine::builder().build()?;
        let http_plugin = HttpServer::new("127.0.0.1:8080".parse()?);
        let runtime_config_plugin = RuntimeConfig::default();

        let host = HostBuilder::new()
            .with_engine(engine)
            .with_plugin(Arc::new(http_plugin))?
            .with_plugin(Arc::new(runtime_config_plugin))?
            .build()?;

        let host = host.start().await?;

        let req = WorkloadStartRequest {
            workload: Workload {
                namespace: "test".to_string(),
                name: "test-workload".to_string(),
                annotations: HashMap::new(),
                service: None,
                components: vec![],
                host_interfaces: vec![],
                volumes: vec![],
            },
        };
        let _res = host.workload_start(req).await?;

        Ok(())
    }
}
