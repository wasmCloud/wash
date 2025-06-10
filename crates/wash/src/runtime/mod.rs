use anyhow::{self, Context as _, ensure};
use sha2::{Digest as _, Sha256};
use std::collections::HashMap;
use std::fmt::Debug;
use std::path::PathBuf;
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
    component::{
        Component, Instance, InstancePre, Linker, ResourceAny, ResourceTable, ResourceType, Val,
        types::ComponentItem,
    },
};
use wasmtime_wasi::{IoImpl, IoView, WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

use crate::runtime::{
    types::{WasmcloudWashCtx, WasmcloudWashImpl, WasmcloudWashView},
    value::{is_host_resource_type, lift, lower},
};

pub mod bindings;
pub(crate) mod types;
mod value;

#[derive(Hash, Eq, PartialEq, Debug, Clone)]
pub enum CapabilityPlugin {
    Blobstore,
}

pub struct Ctx {
    pub wash_ctx: WasmcloudWashCtx,
    pub table: wasmtime::component::ResourceTable,
    pub ctx: WasiCtx,
    pub http: WasiHttpCtx,

    /// `wasi:config/runtime` uses this hashmap to retrieve runtime configuration
    pub runtime_config: Arc<RwLock<HashMap<String, String>>>,
    /// `wasi:blobstore/blobstore` uses this path to store blobs
    pub blobstore_root: Option<PathBuf>,
}

pub struct CtxBuilder {
    ctx: WasiCtx,
    runtime_config: Option<HashMap<String, String>>,
    blobstore_root: Option<PathBuf>,
}

impl CtxBuilder {
    pub fn new() -> Self {
        Self {
            ctx: WasiCtxBuilder::new()
                .args(&["main.wasm"])
                .inherit_stderr()
                .build(),
            runtime_config: None,
            blobstore_root: None,
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

    pub fn with_blobstore_root(mut self, path: Option<PathBuf>) -> Self {
        self.blobstore_root = path;
        self
    }

    pub fn build(self) -> Ctx {
        Ctx {
            runtime_config: Arc::new(RwLock::new(self.runtime_config.unwrap_or_default())),
            blobstore_root: self.blobstore_root,
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
            wash_ctx: WasmcloudWashCtx::default(),
            table: ResourceTable::new(),
            ctx: WasiCtxBuilder::new()
                .args(&["main.wasm"])
                .inherit_stderr()
                .build(),
            http: WasiHttpCtx::new(),
            blobstore_root: None,
            runtime_config: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Debug for Ctx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx")
            .field("wash_ctx", &self.wash_ctx)
            .field("table", &self.table)
            .finish()
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
impl WasmcloudWashView for Ctx {
    fn ctx(&mut self) -> &mut WasmcloudWashCtx {
        &mut self.wash_ctx
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
            add_to_linker_async(linker).context("failed to link `wash`")?;
            // TODO: impl on Ctx
            // host::wash::types::add_to_linker_get_host(linker, |ctx| ctx);
            Ok(())
        },
    )?;

    Ok(component)
}

/// This function plugs a components imports with the exports of other components
/// that are already loaded in the plugin system.
pub fn link_imports_plugin_exports(
    linker: &mut Linker<Ctx>,
    component: &Component,
    plugin_manager: Arc<DevPluginManager>,
) -> anyhow::Result<()> {
    let ty = component.component_type();
    let imports: Vec<_> = ty.imports(component.engine()).collect();
    for (import_name, import_item) in imports.into_iter() {
        match import_item {
            ComponentItem::ComponentInstance(import_instance_ty) => {
                debug!(name = import_name, "processing component instance import");
                let (plugin_component, instance_idx) = {
                    let Some(plugin_component) = plugin_manager.get_component(import_name) else {
                        debug!(name = import_name, "import not found in plugins, skipping");
                        continue;
                    };
                    let Some((ComponentItem::ComponentInstance(_), idx)) =
                        plugin_component.export_index(None, import_name)
                    else {
                        debug!(name = import_name, "skipping non-instance import");
                        continue;
                    };
                    (plugin_component, idx)
                };
                debug!(name = import_name, index = ?instance_idx, "found import at index");

                // Preinstantiate the instance so we can use it later
                let pre = plugin_manager.preinstantiate_instance(linker, import_name)?;

                let mut linker_instance = match linker.instance(import_name) {
                    Ok(i) => i,
                    Err(e) => {
                        debug!(name = import_name, error = %e, "error finding instance in linker, skipping");
                        continue;
                    }
                };

                for (export_name, export_ty) in
                    import_instance_ty.exports(plugin_component.engine())
                {
                    match export_ty {
                        ComponentItem::ComponentFunc(_func_ty) => {
                            let (item, func_idx) = match plugin_component
                                .export_index(Some(&instance_idx), export_name)
                            {
                                Some(res) => res,
                                None => {
                                    debug!(
                                        name = import_name,
                                        fn_name = export_name,
                                        "failed to get export index, skipping"
                                    );
                                    continue;
                                }
                            };
                            ensure!(
                                matches!(item, ComponentItem::ComponentFunc(..)),
                                "expected function export, found other"
                            );
                            debug!(
                                name = import_name,
                                fn_name = export_name,
                                "linking function import"
                            );
                            let import_name: Arc<str> = import_name.into();
                            let export_name: Arc<str> = export_name.into();
                            let pre = pre.clone();
                            let plugin_manager = plugin_manager.clone();
                            linker_instance
                                .func_new_async(
                                    &export_name.clone(),
                                    move |mut store, params, results| {
                                        let import_name = import_name.clone();
                                        let export_name = export_name.clone();
                                        let plugin_manager = plugin_manager.clone();
                                        let pre = pre.clone();
                                        Box::new(async move {
                                            let instance = plugin_manager
                                                .get_or_instantiate_instance(
                                                    &mut store,
                                                    pre,
                                                    &import_name,
                                                )
                                                .await?;

                                            let func = instance
                                                .get_func(&mut store, func_idx)
                                                .context("function not found")?;
                                            trace!(
                                                name = %import_name,
                                                fn_name = %export_name,
                                                ?params,
                                                "lowering params"
                                            );
                                            let mut params_buf = Vec::with_capacity(params.len());
                                            for param_val in params.iter() {
                                                params_buf.push(
                                                    lower(&mut store, param_val)
                                                        .context("failed to lower parameter")?,
                                                );
                                            }
                                            trace!(
                                                name = %import_name,
                                                fn_name = %export_name,
                                                ?params_buf,
                                                "invoking dynamic export"
                                            );

                                            let mut results_buf =
                                                vec![Val::Bool(false); results.len()];
                                            func.call_async(
                                                &mut store,
                                                &params_buf,
                                                &mut results_buf,
                                            )
                                            .await
                                            .context("failed to call function")?;
                                            trace!(
                                                name = %import_name,
                                                fn_name = %export_name,
                                                ?results_buf,
                                                "lifting results"
                                            );
                                            for (i, result_val) in
                                                results_buf.into_iter().enumerate()
                                            {
                                                results[i] = lift(&mut store, result_val)
                                                    .context("failed to lift result")?;
                                            }
                                            trace!(
                                                name = %import_name,
                                                fn_name = %export_name,
                                                ?results,
                                                "invoked dynamic export"
                                            );

                                            func.post_return_async(&mut store)
                                                .await
                                                .context("failed to execute post-return")?;
                                            Ok(())
                                        })
                                    },
                                )
                                .expect("failed to create async func");
                        }
                        ComponentItem::Resource(resource_ty) => {
                            if is_host_resource_type(resource_ty) {
                                debug!(name = import_name, resource = ?resource_ty, "skipping host resource type");
                                continue;
                            }

                            let (item, _idx) = match plugin_component
                                .export_index(Some(&instance_idx), export_name)
                            {
                                Some(res) => res,
                                None => {
                                    debug!(
                                        name = import_name,
                                        resource = export_name,
                                        "failed to get resource index, skipping"
                                    );
                                    continue;
                                }
                            };
                            let ComponentItem::Resource(_) = item else {
                                debug!(
                                    name = import_name,
                                    resource = export_name,
                                    "expected resource export, found non-resource, skipping"
                                );
                                continue;
                            };

                            debug!(name = import_name, resource = export_name, ty = ?resource_ty, "linking resource import");
                            linker_instance
                                .resource(export_name, ResourceType::host::<ResourceAny>(), |_, _| Ok(()))
                                .with_context(|| {
                                    format!(
                                        "failed to define resource import: {import_name}.{export_name}"
                                    )
                                })
                                .unwrap_or_else(|e| {
                                    debug!(name = import_name, resource = export_name, error = %e, "error defining resource import, skipping");
                                });
                        }
                        _ => {
                            trace!(
                                name = import_name,
                                fn_name = export_name,
                                "skipping non-function non-resource import"
                            );
                            continue;
                        }
                    }
                }
            }
            ComponentItem::Resource(resource_ty) => {
                debug!(
                    name = import_name,
                    ty = ?resource_ty,
                    "component import is a resource, which is not supported in this context. skipping."
                );
            }
            _ => continue,
        }
    }
    Ok(())
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
            add_to_linker_async(linker)
                .context("failed to link `wasmcloud:wash` host interface")?;

            // TODO: Use a component that sources env vars for this
            wasmcloud_runtime::capability::config::runtime::add_to_linker(linker, |ctx| ctx)
                .context("failed to link `wasi:config/runtime`")?;

            // Link additional configured plugins
            link_imports_plugin_exports(linker, component, plugin_manager)
                .context("failed to link imports")?;

            Ok(())
        },
    )?;

    Ok(component)
}

//TODO: kill these? I don't think we need it if I impl on the Ctx properly instead of WasmcloudWashView
/// Helper function to properly type annotate the Linker's generic type
fn add_to_linker_async<T: WasmcloudWashView>(linker: &mut Linker<T>) -> anyhow::Result<()> {
    bindings::plugin_host::wasmcloud::wash::types::add_to_linker_get_host(
        linker,
        type_annotate::<T, _>(|t| WasmcloudWashImpl(IoImpl(t))),
    )?;

    Ok(())
}

fn type_annotate<T: WasmcloudWashView, F>(val: F) -> F
where
    F: Fn(&mut T) -> WasmcloudWashImpl<&mut T>,
{
    val
}

// what we basically want is a lookup from instance to Component to start, but then
// to manage a single Instance for that component. E.g. blobstore,container,types should all go to the same instance once instantiated

// I think we can have the plugins managed by a struct, and use methods for properly looking these up

// A hash of the component bytes
type ComponentKey = String;

/// This struct intentionally doesn't derive clone as it should be Arc-ed to share it across threads.
#[derive(Default)]
pub struct DevPluginManager {
    // interface name → component key (deduplicates lookups)
    interface_map: HashMap<String, ComponentKey>,
    // component key → compiled component
    components: HashMap<ComponentKey, Arc<Component>>,

    // component key → instantiated instance (created lazily)
    instances: RwLock<HashMap<ComponentKey, Arc<Instance>>>,
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
        info!(name, "preinstantiating instance for component");
        let component = self.get_component(name).context("component not found")?;
        let instance = linker.instantiate_pre(&component)?;
        Ok(Arc::new(instance))
    }

    pub async fn get_or_instantiate_instance(
        &self,
        store: &mut StoreContextMut<'_, Ctx>,
        pre: Arc<InstancePre<Ctx>>,
        name: &str,
    ) -> anyhow::Result<Arc<Instance>> {
        let key = self
            .interface_map
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("no component implements interface {name}"))?;
        if let Some(instance) = self.instances.read().await.get(key) {
            return Ok(instance.clone());
        }

        let instance = pre.instantiate_async(store.as_context_mut()).await?;
        let instance = Arc::new(instance);

        self.instances
            .write()
            .await
            .insert(key.to_string(), instance.clone());

        Ok(instance)
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
    let exports = component
        .component_type()
        .exports(runtime.engine())
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
    Ok((component, exports))
}

#[cfg(test)]
mod test {
    use http_body_util::BodyExt;
    use tempfile::TempDir;
    use wasmtime_wasi::{DirPerms, FilePerms};
    use wasmtime_wasi_http::{
        bindings::http::types::Scheme,
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
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .init();

        let (runtime, _handle) = new_runtime().await?;

        // This component has a `wasi:blobstore/blobstore` IMPORT
        let wasm = std::fs::read("./tests/fixtures/blobstore_from_fs.wasm")?;
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
                    .preopened_dir(tmp_path, "/tmp", DirPerms::all(), FilePerms::all())
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
        assert_eq!(body_str, "my-container-real");

        Ok(())
    }
}
