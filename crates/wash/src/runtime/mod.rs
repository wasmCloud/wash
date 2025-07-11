use anyhow::{self, Context as _};
use sha2::{Digest as _, Sha256};
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use std::thread::JoinHandle;
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace, warn};
use wasmcloud_runtime::Runtime;
use wasmcloud_runtime::capability::config::runtime::ConfigError;
use wasmcloud_runtime::capability::logging::logging::Level;
use wasmcloud_runtime::component::BaseCtx;
use wasmtime::{
    AsContextMut as _, StoreContextMut,
    component::{Component, Instance, InstancePre, Linker, ResourceTable, types::ComponentItem},
};
use wasmtime_wasi::{IoView, WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

use crate::runtime::{bindings::plugin_host, wasm::link_imports_plugin_exports};

pub mod bindings;
pub(crate) mod types;
mod wasm;

pub struct Ctx {
    /// Unique identifier for this component context. Primarily used for logging, debugging, and
    /// plugin instance management.
    pub id: String,
    pub table: wasmtime::component::ResourceTable,
    pub ctx: WasiCtx,
    pub http: WasiHttpCtx,

    /// `wasi:config/runtime` uses this hashmap to retrieve runtime configuration
    pub runtime_config: Arc<RwLock<HashMap<String, String>>>,
}

pub struct CtxBuilder {
    ctx: WasiCtx,
    runtime_config: Option<HashMap<String, String>>,
}

impl CtxBuilder {
    pub fn new() -> Self {
        Self {
            ctx: WasiCtxBuilder::new()
                .args(&["main.wasm"])
                .inherit_stderr()
                .build(),
            runtime_config: None,
        }
    }

    pub fn with_wasi_ctx(mut self, ctx: WasiCtx) -> Self {
        self.ctx = ctx;
        self
    }

    pub fn with_runtime_config(mut self, config: HashMap<String, String>) -> Self {
        self.runtime_config = Some(config);
        self
    }

    pub fn build(self) -> Ctx {
        Ctx {
            runtime_config: Arc::new(RwLock::new(self.runtime_config.unwrap_or_default())),
            ctx: self.ctx,
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
        CtxBuilder::new()
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
            runtime_config: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Debug for Ctx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx").field("table", &self.table).finish()
    }
}
impl BaseCtx for Ctx {}
impl IoView for Ctx {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}
impl WasiView for Ctx {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.ctx
    }
}
impl WasiHttpView for Ctx {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }
}

impl wasmcloud_runtime::capability::logging::logging::Host for Ctx {
    async fn log(&mut self, level: Level, context: String, message: String) -> anyhow::Result<()> {
        match level {
            // TODO: Include some kind of information about the component as an attribute
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

/// Implementation of `wasi:config/runtime` using an in-memory key-value store
impl wasmcloud_runtime::capability::config::runtime::Host for Ctx {
    async fn get(&mut self, key: String) -> anyhow::Result<Result<Option<String>, ConfigError>> {
        Ok(Ok(self.runtime_config.read().await.get(&key).cloned()))
    }

    async fn get_all(&mut self) -> anyhow::Result<Result<Vec<(String, String)>, ConfigError>> {
        Ok(Ok(self
            .runtime_config
            .read()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()))
    }
}

#[derive(Debug, Clone)]
pub struct ComponentHandler {
    _priv: (),
}

pub async fn new_runtime() -> anyhow::Result<(Runtime, JoinHandle<Result<(), ()>>)> {
    wasmcloud_runtime::RuntimeBuilder::new().build()
}

/// Creates a WebAssembly component from bytes and compiles using [Runtime].
///
/// This function is used to prepare a component for use in the plugin system, adding in core
/// WASI interfaces with [wasmtime_wasi::add_to_linker_async] and linking the `wash` host
/// interface.
pub async fn prepare_component_plugin(
    runtime: &Runtime,
    wasm: &[u8],
) -> anyhow::Result<wasmcloud_runtime::component::CustomCtxComponent<Ctx>> {
    let component = wasmcloud_runtime::component::CustomCtxComponent::new_with_linker_minimal(
        runtime,
        wasm,
        |linker, _component| {
            wasmtime_wasi::add_to_linker_async(linker)
                .context("failed to link core WASI interfaces")?;
            wasmcloud_runtime::capability::logging::logging::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasi:logging/logging`")?;

            // Add wash plugin host
            plugin_host::PluginHost::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasmcloud:wash` host interface")?;
            // TODO: impl on Ctx
            // host::wash::types::add_to_linker_get_host(linker, |ctx| ctx);
            Ok(())
        },
    )?;

    Ok(component)
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

            plugin_host::PluginHost::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasmcloud:wash` host interface")?;

            // Link additional configured plugins
            link_imports_plugin_exports(linker, component, plugin_manager)
                .context("failed to link imports")?;

            Ok(())
        },
    )?;

    Ok(component)
}

// A hash of the component bytes
type ComponentKey = String;

/// This struct intentionally doesn't derive clone as it should be Arc-ed to share it across threads.
#[derive(Default)]
pub struct DevPluginManager {
    // interface name → component key (deduplicates lookups)
    interface_map: HashMap<String, ComponentKey>,
    // component key → compiled component
    components: HashMap<ComponentKey, Arc<Component>>,
    // Instances is keyed differently than components, as it's keyed by a specific Instance of a component
    // This allows us to instantiate the same component multiple times with different configurations.
    // component id → component key -> instantiated instance (created lazily)
    instances: RwLock<HashMap<String, HashMap<ComponentKey, Arc<Instance>>>>,
}

impl DevPluginManager {
    pub fn register_plugin(&mut self, runtime: &Runtime, wasm: &[u8]) -> anyhow::Result<()> {
        let (component, exports) = initialize_plugin_exports(runtime, wasm)?;

        let key = format!("{:x}", Sha256::digest(wasm));
        for (name, item) in exports {
            if let ComponentItem::ComponentInstance(_) = item {
                // Register the interface name to the component key
                if self.interface_map.contains_key(&name) {
                    anyhow::bail!("another plugin already implements the interface '{name}'");
                }
                self.interface_map.insert(name.clone(), key.clone());
            } else {
                warn!(name, "exported item is not a component instance, skipping");
            }
        }

        self.components.insert(key.clone(), Arc::new(component));

        Ok(())
    }

    /// Looks up a component for an interface name, or returns None if not found.
    pub fn get_component(&self, name: &str) -> Option<Arc<Component>> {
        let key = self.interface_map.get(name)?;
        self.components.get(key).cloned()
    }

    pub fn preinstantiate_instance(
        &self,
        linker: &Linker<Ctx>,
        name: &str,
    ) -> anyhow::Result<Arc<InstancePre<Ctx>>> {
        trace!(name, "preinstantiating instance for component");
        let component = self.get_component(name).context("component not found")?;
        let instance = linker.instantiate_pre(&component)?;
        Ok(Arc::new(instance))
    }

    pub async fn get_or_instantiate_instance(
        &self,
        store: &mut StoreContextMut<'_, Ctx>,
        // TODO: Instances don't need to be Arc-ed, they're already cheaply cloneable
        pre: Arc<InstancePre<Ctx>>,
        name: &str,
    ) -> anyhow::Result<Arc<Instance>> {
        let key = self
            .interface_map
            .get(name)
            .context("component not found")?;

        let id = store.data().id.clone();

        if let Some(instances) = self.instances.read().await.get(&id) {
            if let Some(instance) = instances.get(key) {
                trace!(id, "found existing instance for component");
                return Ok(instance.clone());
            }
        }

        let instance = Arc::new(pre.instantiate_async(store.as_context_mut()).await?);
        trace!(id, key, "instantiated new instance for component");
        self.instances
            .write()
            .await
            .entry(id.to_string())
            .and_modify(|e| {
                e.insert(key.clone(), instance.clone());
            })
            .or_insert(HashMap::from([(key.clone(), instance.clone())]));

        Ok(instance)
    }

    /// Clone this DevManager and clear the instances map, removing all instantiated plugin components.
    ///
    /// Must be done before each instantiation of a component to ensure that the
    /// plugins are using the same store as the instantiated component.
    pub fn clear_instances(self: Arc<Self>) -> Arc<Self> {
        Arc::new(Self {
            interface_map: self.interface_map.clone(),
            components: self.components.clone(),
            instances: RwLock::new(HashMap::new()),
        })
    }
}

/// Inspects a WebAssembly component and returns its exports, specifically looking for
/// component instances that can be used as plugins. More could be implemented in terms of
/// exports, but `wash` primarily supports using component instances as plugins.
fn initialize_plugin_exports(
    runtime: &Runtime,
    wasm: &[u8],
) -> anyhow::Result<(Component, Vec<(String, ComponentItem)>)> {
    let component = wasmtime::component::Component::new(runtime.engine(), wasm)
        .context("failed to create component from wasm")?;
    let exports =
        plugin_exports(&component).context("failed to get plugin exports from component")?;
    Ok((component, exports))
}

fn plugin_exports(component: &Component) -> anyhow::Result<Vec<(String, ComponentItem)>> {
    let exports = component
        .component_type()
        .exports(component.engine())
        .filter_map(|(name, item)| {
            // An instance in this case is something like `wasi:blobstore/blobstore@0.2.0-draft`, or an
            // entire interface that's exported.
            if matches!(item, ComponentItem::ComponentInstance(_)) {
                Some((name.to_string(), item))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    Ok(exports)
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

    use crate::runtime::bindings::{dev::Dev, plugin_guest::PluginGuest};

    use super::*;

    use tokio;

    #[tokio::test]
    async fn can_instantiate_plugin() -> anyhow::Result<()> {
        // Built from ./plugins/blobstore-filesystem
        let wasm = std::fs::read("./tests/fixtures/blobstore_filesystem.wasm")?;

        let (runtime, _handle) = new_runtime().await?;
        let base_ctx = Ctx::default();

        let component = prepare_component_plugin(&runtime, &wasm).await?;
        let mut store = component.new_store(base_ctx);

        let instance = component
            .instance_pre()
            .instantiate_async(&mut store)
            .await?;
        let wash_plugin = PluginGuest::new(&mut store, &instance)?;
        let _ = wash_plugin
            .wasmcloud_wash_plugin()
            .call_info(&mut store)
            .await?;

        Ok(())
    }

    #[tokio::test]
    async fn can_instantiate_http_component() -> anyhow::Result<()> {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .try_init();
        let wasm = std::fs::read("./tests/fixtures/http_hello_world_rust.wasm")?;

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
        let wasm = std::fs::read("./tests/fixtures/http_blobstore.wasm")?;
        let blobstore_plugin = std::fs::read("./tests/fixtures/blobstore_filesystem.wasm")?;

        let mut plugin_manager = DevPluginManager::default();
        plugin_manager
            .register_plugin(&runtime, &blobstore_plugin)
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
