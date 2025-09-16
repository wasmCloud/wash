use anyhow::{self, Context as _};
use std::collections::HashMap;
use std::fmt::Debug;
use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;
use tokio::{process::Child, sync::RwLock};
use tracing::{debug, error, info, trace, warn};
use wasmtime::component::ResourceTable;
use wasmtime_wasi::{IoView, WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

use crate::{dev::DevPluginManager, plugin::PluginComponent};

pub mod bindings;
pub mod plugin;
pub mod tracing_streams;

#[cfg(test)]
mod test {
    use http_body_util::BodyExt;
    use tempfile::TempDir;
    use wasmtime_wasi::{DirPerms, FilePerms};
    use wasmtime_wasi_http::{
        bindings::http::types::{ErrorCode, Scheme},
        body::{HostIncomingBody, HyperIncomingBody},
        types::HostIncomingRequest,
    };

    use crate::{dev::DevPluginManager, runtime::bindings::dev::Dev};

    use super::*;

    use tokio;

    #[tokio::test]
    async fn can_instantiate_plugin() -> anyhow::Result<()> {
        // Built from ./plugins/blobstore-filesystem
        let wasm = tokio::fs::read("./tests/fixtures/blobstore_filesystem.wasm").await?;

        let (runtime, _handle) = new_runtime().await?;

        let component = prepare_component_plugin(&runtime, &wasm, None).await?;
        let metadata = component.call_info(Ctx::default()).await?;

        assert_eq!(metadata.name, "blobstore-filesystem");

        Ok(())
    }

    #[tokio::test]
    async fn can_instantiate_http_component() -> anyhow::Result<()> {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .try_init();
        let wasm = tokio::fs::read("./tests/fixtures/http_hello_world_rust.wasm").await?;

        let (runtime, _handle) = new_runtime().await?;
        let base_ctx = Ctx::default();

        let component = prepare_component_dev(&runtime, &wasm, Arc::default()).await?;
        let mut store = component.new_store(base_ctx);

        let instance = component
            .instance_pre()
            .instantiate_async(&mut store)
            .await?;

        let http_instance = Dev::new(&mut store, &instance)
            .context("failed to pre-instantiate `wasi:http/incoming-handler`")?;

        let data = store.data_mut();
        let request: ::http::Request<wasmtime_wasi_http::body::HyperIncomingBody> =
            http::Request::builder()
                .uri("http://localhost:8080")
                .body(HyperIncomingBody::default())
                .context("failed to create request")?;

        let (parts, body) = request.into_parts();
        let body = HostIncomingBody::new(body, std::time::Duration::from_millis(600 * 1000));
        let wasmtime_scheme = Scheme::Http;
        let incoming_req = HostIncomingRequest::new(data, parts, wasmtime_scheme, Some(body))?;
        let request = data.table().push(incoming_req)?;

        let (tx, rx) = tokio::sync::oneshot::channel();
        let response = data
            .new_response_outparam(tx)
            .context("failed to create response")?;

        http_instance
            .wasi_http_incoming_handler()
            .call_handle(&mut store, request, response)
            .await?;

        let resp = rx
            .await
            .context("failed to receive response (inner)")?
            .context("failed to receive response (outer")?;

        assert!(resp.status() == 200);
        let body = resp
            .collect()
            .await
            .context("failed to collect body bytes")?
            .to_bytes();
        let body_str = String::from_utf8_lossy(&body);
        assert_eq!(body_str, "Hello from Rust!\n");

        Ok(())
    }

    #[tokio::test]
    async fn can_instantiate_blobstore_component() -> anyhow::Result<()> {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .try_init();

        let (runtime, _handle) = new_runtime().await?;

        // This component has a `wasi:blobstore/blobstore` IMPORT
        let wasm = tokio::fs::read("./tests/fixtures/http_blobstore.wasm").await?;
        let blobstore_plugin =
            tokio::fs::read("./tests/fixtures/blobstore_filesystem.wasm").await?;

        let mut plugin_manager = DevPluginManager::default();
        plugin_manager
            .register_plugin(prepare_component_plugin(&runtime, &blobstore_plugin, None).await?)
            .context("failed to register blobstore plugin")?;
        let component = prepare_component_dev(&runtime, &wasm, Arc::new(plugin_manager)).await?;

        let tmp_dir =
            TempDir::new().context("failed to create temporary directory for blobstore")?;
        let tmp_path = tmp_dir.path().join("blobstore_test");
        tokio::fs::create_dir_all(&tmp_path).await?;

        let base_ctx = Ctx::builder("test_component".to_string())
            .with_wasi_ctx(
                WasiCtxBuilder::new()
                    .preopened_dir(&tmp_path, "/tmp", DirPerms::all(), FilePerms::all())
                    .context("failed to create preopened dir")?
                    .stdout(crate::runtime::tracing_streams::TracingStream::stdout(
                        "test_component".to_string(),
                    ))
                    .stderr(crate::runtime::tracing_streams::TracingStream::stderr(
                        "test_component".to_string(),
                    ))
                    .build(),
            )
            .build();

        let mut store = component.new_store(base_ctx);

        let instance = component
            .instance_pre()
            .instantiate_async(&mut store)
            .await?;

        let http_instance = Dev::new(&mut store, &instance)
            .context("failed to pre-instantiate `wasi:http/incoming-handler`")?;

        let data = store.data_mut();
        let body = HyperIncomingBody::new(
            // One gigabyte of data. Larger than this will hang on the get_data TODO: investigate
            http_body_util::Full::new(hyper::body::Bytes::from("0123456789".repeat(100_000)))
                .map_err(|e| ErrorCode::InternalError(Some(e.to_string())))
                .boxed(),
        );
        let request: ::http::Request<wasmtime_wasi_http::body::HyperIncomingBody> =
            http::Request::builder()
                .uri("http://localhost:8080")
                .body(body)
                .context("failed to create request")?;

        let (parts, body) = request.into_parts();
        let body = HostIncomingBody::new(body, std::time::Duration::from_millis(600 * 1000));
        let wasmtime_scheme = Scheme::Http;
        let incoming_req = HostIncomingRequest::new(data, parts, wasmtime_scheme, Some(body))?;
        let request = data.table().push(incoming_req)?;

        let (tx, rx) = tokio::sync::oneshot::channel();
        let response = data
            .new_response_outparam(tx)
            .context("failed to create response")?;

        http_instance
            .wasi_http_incoming_handler()
            .call_handle(&mut store, request, response)
            .await?;

        let resp = rx
            .await
            .context("failed to receive response (inner)")?
            .context("failed to receive response (outer")?;

        assert!(resp.status() == 200);
        let body = resp
            .collect()
            .await
            .context("failed to collect body bytes")?
            .to_bytes();
        assert_eq!(body.len(), 1_000_000);

        eprintln!("roundtrip streamed data: {}", body.len());

        Ok(())
    }
}
