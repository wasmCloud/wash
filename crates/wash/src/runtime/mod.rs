use anyhow::{self, Context as _};
use std::collections::HashMap;
use std::fmt::Debug;
use std::path::Path;
use std::sync::Arc;
use std::thread::JoinHandle;
use tokio::{process::Child, sync::RwLock};
use tracing::{debug, error, info, trace, warn};
use wasmcloud_runtime::Runtime;
use wasmcloud_runtime::capability::config::runtime::ConfigError;
use wasmcloud_runtime::capability::logging::logging::Level;
use wasmcloud_runtime::component::BaseCtx;
use wasmtime::component::ResourceTable;
use wasmtime_wasi::{IoView, WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

use crate::{
    dev::DevPluginManager, plugin::PluginComponent, runtime::wasm::link_imports_plugin_exports,
};

pub mod bindings;
pub mod plugin;
pub mod tracing_streams;
mod wasm;

/// The context for a component store and linker, providing access to implementations of:
/// - wasi@0.2 interfaces
/// - `wasi:logging/logging`
/// - `wasi:config/runtime`
/// - `wasi:http`
/// - `wasmcloud:wash/plugin`
pub struct Ctx {
    /// Unique identifier for this component context. Primarily used for logging, debugging, and
    /// plugin instance management. This is a [uuid::Uuid::now_v7] string.
    pub id: String,
    /// The resource table used to manage resources in the Wasmtime store.
    pub table: wasmtime::component::ResourceTable,
    /// The WASI context used to provide WASI functionality to the component.
    pub ctx: WasiCtx,
    /// The HTTP context used to provide HTTP functionality to the component.
    pub http: WasiHttpCtx,

    /// Powers the built-in implementation of `wasi:config/runtime`
    pub runtime_config: Arc<RwLock<HashMap<String, String>>>,
    /// Stores the handles to background processes spawned by host_exec_background. Once this
    /// context struct is dropped the processes will be removed
    pub background_processes: Arc<RwLock<Vec<Child>>>,
}

/// Helper struct to build a [`Ctx`] with a builder pattern
pub struct CtxBuilder {
    ctx: WasiCtx,
    runtime_config: Option<Arc<RwLock<HashMap<String, String>>>>,
    background_processes: Option<Arc<RwLock<Vec<Child>>>>,
    component_name: Option<String>,
}

impl CtxBuilder {
    pub fn new() -> Self {
        Self {
            ctx: WasiCtxBuilder::new().args(&["main.wasm"]).build(),
            runtime_config: None,
            background_processes: None,
            component_name: None,
        }
    }

    pub fn with_wasi_ctx(mut self, ctx: WasiCtx) -> Self {
        self.ctx = ctx;
        self
    }

    /// Sets the runtime configuration for the context. If you want to modify the configuration after
    /// the context is build, you can use [`CtxBuilder::with_runtime_config_arc`] instead.
    pub fn with_runtime_config(mut self, config: HashMap<String, String>) -> Self {
        self.runtime_config = Some(Arc::new(RwLock::new(config)));
        self
    }

    /// Sets the runtime configuration for the context using an Arc-wrapped RwLock. This allows for modifying
    /// the configuration after the context is built, as the Arc can be cloned and shared across threads.
    ///
    /// Take care to avoid deadlocks when using this method, as the RwLock is shared across threads.
    pub fn with_runtime_config_arc(mut self, config: Arc<RwLock<HashMap<String, String>>>) -> Self {
        self.runtime_config = Some(config);
        self
    }

    /// Sets the background processes list for the context using an Arc-wrapped RwLock. This allows sharing
    /// the background processes list across multiple Ctx instances, typically from CliContext.
    pub fn with_background_processes(mut self, processes: Arc<RwLock<Vec<Child>>>) -> Self {
        self.background_processes = Some(processes);
        self
    }

    /// Sets the component name, which will enable tracing-based stdout/stderr logging
    pub fn with_component_name(mut self, name: String) -> Self {
        self.component_name = Some(name);
        self
    }

    pub fn build(self) -> Ctx {
        let ctx = if let Some(component_name) = self.component_name {
            // Use tracing streams when component name is provided
            WasiCtxBuilder::new()
                .args(&["main.wasm"])
                .stdout(crate::runtime::tracing_streams::TracingStream::stdout(
                    component_name.clone(),
                ))
                .stderr(crate::runtime::tracing_streams::TracingStream::stderr(
                    component_name,
                ))
                .build()
        } else {
            // Fall back to existing behavior when no component name
            self.ctx
        };

        Ctx {
            ctx,
            runtime_config: self.runtime_config.unwrap_or_default(),
            background_processes: self.background_processes.unwrap_or_default(),
            ..Default::default()
        }
    }
}

impl Default for CtxBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Ctx {
    pub fn builder() -> CtxBuilder {
        CtxBuilder::default()
    }
}

impl Default for Ctx {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::now_v7().to_string(),
            table: ResourceTable::new(),
            ctx: WasiCtxBuilder::new()
                .args(&["main.wasm"])
                .inherit_stderr()
                .build(),
            http: WasiHttpCtx::new(),
            runtime_config: Arc::default(),
            background_processes: Arc::default(),
        }
    }
}

impl Debug for Ctx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx")
            .field("id", &self.id)
            .field("table", &self.table)
            .field("http", &self.http)
            .field("runtime_config", &self.runtime_config)
            .field("background_processes", &self.background_processes)
            .finish()
    }
}
// Implement the base context for wasmcloud_runtime
impl BaseCtx for Ctx {}
// Implement IoView for resources
impl IoView for Ctx {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}
// Implement WasiView for wasi@0.2 interfaces
impl WasiView for Ctx {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.ctx
    }
}
// Implement WasiHttpView for wasi:http@0.2
impl WasiHttpView for Ctx {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }
}

// TODO(IMPORTANT): Remove in favor of stdout/stderr logging
// Implementation of `wasi:logging/logging` using the `tracing` crate
// DEPRECATED: Use stdout/stderr with component context instead of wasi:logging
impl wasmcloud_runtime::capability::logging::logging::Host for Ctx {
    async fn log(&mut self, level: Level, context: String, message: String) -> anyhow::Result<()> {
        // Emit deprecation warning on first use
        static DEPRECATION_WARNED: std::sync::Once = std::sync::Once::new();
        DEPRECATION_WARNED.call_once(|| {
            warn!("wasi:logging is deprecated - use stdout/stderr for component logging instead");
        });

        match level {
            Level::Critical => error!(ctx = context, "{message}"),
            Level::Error => error!(ctx = context, "{message}"),
            Level::Warn => warn!(ctx = context, "{message}"),
            Level::Info => info!(ctx = context, "{message}"),
            Level::Debug => debug!(ctx = context, "{message}"),
            Level::Trace => trace!(ctx = context, "{message}"),
        }
        Ok(())
    }
}

// Implementation of `wasi:config/runtime` using an in-memory key-value store
impl wasmcloud_runtime::capability::config::runtime::Host for Ctx {
    async fn get(&mut self, key: String) -> anyhow::Result<Result<Option<String>, ConfigError>> {
        Ok(Ok(self.runtime_config.read().await.get(&key).cloned()))
    }

    async fn get_all(&mut self) -> anyhow::Result<Result<Vec<(String, String)>, ConfigError>> {
        Ok(Ok(self
            .runtime_config
            .read()
            .await
            .clone()
            .into_iter()
            .collect()))
    }
}

/// Create a new [`wasmcloud_runtime::Runtime`], returning the runtime and a handle to the runtime thread.
pub async fn new_runtime() -> anyhow::Result<(Runtime, JoinHandle<Result<(), ()>>)> {
    wasmcloud_runtime::RuntimeBuilder::new().build()
}

/// Creates a WebAssembly component from bytes and compiles using [Runtime].
///
/// This function is used to prepare a component for use in the plugin system, adding in core
/// WASI interfaces with [wasmtime_wasi::add_to_linker_async] and linking the wash host
/// interface.
pub async fn prepare_component_plugin(
    runtime: &Runtime,
    wasm: &[u8],
    data_dir: Option<&Path>,
) -> anyhow::Result<PluginComponent> {
    let component = wasmcloud_runtime::component::CustomCtxComponent::new_with_linker_minimal(
        runtime,
        wasm,
        |linker, _component| {
            // Wasi 0.2 interfaces
            wasmtime_wasi::add_to_linker_async(linker)
                .context("failed to link core WASI interfaces")?;
            // Logging
            wasmcloud_runtime::capability::logging::logging::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasi:logging/logging`")?;
            // Runtime config
            wasmcloud_runtime::capability::config::runtime::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasi:config/runtime`")?;
            // HTTP
            wasmtime_wasi_http::add_only_http_to_linker_async(linker)
                .context("failed to link `wasi:http`")?;
            // Plugin host
            bindings::plugin::WashPlugin::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasmcloud:wash/plugin`")?;
            Ok(())
        },
    )?;

    PluginComponent::new(component, data_dir).await
}

/// Creates a WebAssembly component from bytes and compiles using [Runtime].
///
/// This function is used to prepare a component for use in the plugin system, adding in the following interfaces:
/// - WASI 0.2 interfaces with [wasmtime_wasi::add_to_linker_async]
/// - `wasi:http` with [wasmtime_wasi_http::add_only_http_to_linker_async].
/// - `wasi:config/runtime` with [wasmcloud_runtime::capability::config::runtime::add_to_linker].
/// - `wasi:logging/logging` with [wasmcloud_runtime::capability::logging::logging::add_to_linker].
///
/// This will result in an error if the component does not implement the `wasi:http/incoming-handler` interface, or if it
/// attempts to import interfaces that are not listed above.
pub async fn prepare_component_dev(
    runtime: &Runtime,
    wasm: &[u8],
    plugin_manager: Arc<DevPluginManager>,
) -> anyhow::Result<wasmcloud_runtime::component::CustomCtxComponent<Ctx>> {
    let component = wasmcloud_runtime::component::CustomCtxComponent::new_with_linker_minimal(
        runtime,
        wasm,
        |linker, component| {
            // Core builtins:
            // - WASI 0.2 interfaces
            // - `wasi:http`
            // - `wasi:logging`
            // - `wasmcloud:wash`
            wasmtime_wasi::add_to_linker_async(linker)
                .context("failed to link core WASI interfaces")?;
            wasmtime_wasi_http::add_only_http_to_linker_async(linker)
                .context("failed to link `wasi:http`")?;
            wasmcloud_runtime::capability::logging::logging::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasi:logging/logging`")?;
            wasmcloud_runtime::capability::config::runtime::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasi:config/runtime`")?;

            bindings::plugin::WashPlugin::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasmcloud:wash` host interface")?;

            // Link additional configured plugins
            link_imports_plugin_exports(linker, component, plugin_manager)
                .context("failed to link imports")?;

            Ok(())
        },
    )?;

    Ok(component)
}

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

        let base_ctx = Ctx::builder()
            .with_wasi_ctx(
                WasiCtxBuilder::new()
                    .preopened_dir(&tmp_path, "/tmp", DirPerms::all(), FilePerms::all())
                    .context("failed to create preopened dir")?
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
