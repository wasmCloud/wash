//! This module is primarily concerned with converting an [`UnresolvedWorkload`] into a [`ResolvedWorkload`] by
//! resolving all components and their dependencies.
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use anyhow::{Context as _, bail, ensure};
use tokio::sync::RwLock;
use tracing::{debug, trace, warn};
use wasmtime::component::{
    Component, Instance, Linker, ResourceAny, ResourceType, Val, types::ComponentItem,
};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder};

use crate::{
    engine::{
        ctx::Ctx,
        value::{lift, lower},
    },
    plugin::HostPlugin,
    types::{Service, VolumeMount},
    wit::{WitInterface, WitWorld},
};

/// A [`WorkloadComponent`] is a component that is part of a workload.
///
/// It contains the actual [`Component`] that can be instantiated,
/// the [`Linker`] for creating stores and instances, the available
/// [`VolumeMount`]s to be passed as filesystem preopens, and the
/// full list of [`HostPlugin`]s that the component depends on.
#[derive(Clone)]
pub struct WorkloadComponent {
    /// The unique identifier for this component
    id: String,
    /// The unique identifier for the workload this component belongs to
    workload_id: String,
    component: Component,
    linker: Linker<Ctx>,
    volume_mounts: Vec<(PathBuf, VolumeMount)>,
    pool_size: usize,
    max_invocations: usize,
    plugins: Option<HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>>,
}

impl WorkloadComponent {
    /// Create a new [`WorkloadComponent`] with the given workload ID,
    /// wasmtime [`Component`], [`Linker`], volume mounts, and instance limits.
    pub fn new(
        workload_id: String,
        component: Component,
        linker: Linker<Ctx>,
        volume_mounts: Vec<(PathBuf, VolumeMount)>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            workload_id,
            component,
            linker,
            volume_mounts,
            plugins: None,
            // TODO: Implement pooling and instance limits
            pool_size: 0,
            max_invocations: 0,
        }
    }

    /// Extracts the [`ComponentItem::ComponentInstance`]s that the given [`Component`] exports.
    ///
    /// Primarily used when resolving component dependencies and linking since the [`ComponentItem`]
    /// contains valuable type information.
    pub fn component_exports(&self) -> anyhow::Result<Vec<(String, ComponentItem)>> {
        Ok(self
            .component
            .component_type()
            .exports(self.component.engine())
            .filter_map(|(name, item)| {
                // An instance in this case is something like `wasi:blobstore/blobstore@0.2.0-draft`, or an
                // entire interface that's exported.
                if matches!(item, ComponentItem::ComponentInstance(_)) {
                    Some((name.to_string(), item))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>())
    }

    /// Computes and returns the [`WitWorld`] of this component for convenient
    /// comparison when resolving component dependencies.
    pub fn world(&self) -> anyhow::Result<WitWorld> {
        let mut imports = HashMap::new();
        let mut exports = HashMap::new();

        // Iterate over imports, merging interfaces when namespace:package@version matches
        for (import_name, import_item) in self
            .component
            .component_type()
            .imports(self.component.engine())
        {
            if let ComponentItem::ComponentInstance(_) = import_item {
                let interface = WitInterface::from(import_name);
                let k = interface.instance();
                imports
                    .entry(k)
                    .and_modify(|existing: &mut WitInterface| {
                        existing.merge(&interface);
                    })
                    .or_insert(interface);
            } else {
                debug!(
                    import_name,
                    "imported item is not a component instance, skipping"
                );
            }
        }

        // Iterate over exports, merging interfaces when namespace:package@version matches
        for (export_name, export_item) in self
            .component
            .component_type()
            .exports(self.component.engine())
        {
            if let ComponentItem::ComponentInstance(_) = export_item {
                // Register the interface name to the component key
                let interface = WitInterface::from(export_name);
                let k = interface.instance();
                exports
                    .entry(k)
                    .and_modify(|existing: &mut WitInterface| {
                        existing.merge(&interface);
                    })
                    .or_insert(interface);
            } else {
                debug!(
                    export_name,
                    "exported item is not a component instance, skipping"
                );
            }
        }

        Ok(WitWorld {
            imports: imports.into_values().collect(),
            exports: exports.into_values().collect(),
        })
    }

    /// Adds a [`HostPlugin`] to the component
    pub fn add_plugin(&mut self, id: &'static str, plugin: Arc<dyn HostPlugin + Send + Sync>) {
        if let Some(ref mut plugins) = self.plugins {
            plugins.insert(id, plugin);
        } else {
            let mut plugins = HashMap::new();
            plugins.insert(id, plugin);
            self.plugins = Some(plugins);
        }
    }

    /// Replaces all plugins for this component with the provided set.
    ///
    /// # Arguments
    /// * `plugins` - A HashMap of plugin IDs to plugin instances
    pub fn with_plugins(
        &mut self,
        plugins: HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>,
    ) {
        self.plugins = Some(plugins);
    }

    /// Returns the unique identifier for this component.
    ///
    /// # Returns
    /// The component's UUID string.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the ID of the workload this component belongs to.
    ///
    /// # Returns
    /// The parent workload's ID string.
    pub fn workload_id(&self) -> &str {
        &self.workload_id
    }

    /// Returns a reference to the plugins associated with this component.
    ///
    /// # Returns
    /// An optional reference to the plugin HashMap, None if no plugins are configured.
    pub fn plugins(&self) -> &Option<HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>> {
        &self.plugins
    }

    /// Returns a reference to the wasmtime engine used to compile this component
    pub fn engine(&self) -> &wasmtime::Engine {
        self.component.engine()
    }

    /// Returns a mutable reference to the component's linker.
    ///
    /// The linker is used to wire up imports and exports for the component.
    ///
    /// # Returns
    /// A mutable reference to the wasmtime `Linker`.
    pub fn linker(&mut self) -> &mut Linker<Ctx> {
        &mut self.linker
    }
}

impl std::fmt::Debug for WorkloadComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkloadComponent")
            .field("id", &self.id)
            .field("workload_id", &self.workload_id)
            .field("volume_mounts", &self.volume_mounts)
            .field("pool_size", &self.pool_size)
            .field("max_invocations", &self.max_invocations)
            .finish()
    }
}

/// A fully resolved workload ready for execution.
///
/// A `ResolvedWorkload` contains all components that have been validated,
/// bound to plugins, and had their dependencies resolved. This is the final
/// state of a workload before execution.
#[derive(Debug, Clone)]
pub struct ResolvedWorkload {
    /// The unique identifier of the workload, created with [uuid::Uuid::new_v4]
    id: String,
    /// The name of the workload
    name: String,
    /// The namespace of the workload
    namespace: String,
    /// All components in the workload. This is behind a `RwLock` to support mutable
    /// access to the component linkers.
    components: Arc<RwLock<HashMap<String, WorkloadComponent>>>,
    // TODO: implement service
}

impl ResolvedWorkload {
    async fn link_components(&self) -> anyhow::Result<()> {
        // A map from component ID to its exported interfaces
        let mut interface_map = HashMap::new();

        // Determine available component exports to link to the rest of the workload
        for c in self.components.read().await.values() {
            let exported_instances = c.component_exports()?;
            for (name, item) in exported_instances {
                // TODO(#11): It's probably a good idea to skip registering wasi@0.2 interfaces
                match name.split_once('@') {
                    Some(("wasmcloud:wash/plugin", _)) => {
                        trace!(name, "skipping internal plugin export");
                        continue;
                    }
                    None => {
                        if name == "wasmcloud:wash/plugin" {
                            trace!(name, "skipping internal plugin export");
                            continue;
                        }
                    }
                    _ => {}
                }
                if let ComponentItem::ComponentInstance(_) = item {
                    // Register the interface name to the component key
                    if interface_map.contains_key(&name) {
                        anyhow::bail!(
                            "another component already implements the interface '{name}'"
                        );
                    }
                    interface_map.insert(name.clone(), c.id().to_string());
                } else {
                    warn!(name, "exported item is not a component instance, skipping");
                }
            }
        }

        // TODO: Link exports to service

        self.resolve_workload_imports(&interface_map).await?;

        Ok(())
    }

    // TODO: Good gravy please un-nest this
    /// This function plugs a components imports with the exports of other components
    /// that are already loaded in the plugin system.
    async fn resolve_workload_imports(
        &self,
        interface_map: &HashMap<String, String>,
    ) -> anyhow::Result<()> {
        // TODO: no clone? just need a read copy basically
        let all_components = self.components.read().await.clone();
        for workload_component in self.components.write().await.values_mut() {
            let component = workload_component.component.clone();
            let ty = component.component_type();
            let linker = workload_component.linker();
            let imports: Vec<_> = ty.imports(component.engine()).collect();

            // TODO: some kind of shared import_name -> component registry. need to remove when new store
            // store id, instance, import_name. That will keep the instance properly unique
            let instance: Arc<RwLock<Option<(String, Instance)>>> = Arc::default();
            for (import_name, import_item) in imports.into_iter() {
                match import_item {
                    ComponentItem::ComponentInstance(import_instance_ty) => {
                        trace!(name = import_name, "processing component instance import");
                        let (plugin_component, instance_idx) = {
                            let Some(exporter_component) = interface_map.get(import_name) else {
                                // TODO: error because unsatisfied import, if there's no available
                                // export then it's an unresolvable workload
                                trace!(
                                    name = import_name,
                                    "import not found in component exports, skipping"
                                );
                                continue;
                            };
                            let Some(plugin_component) = all_components.get(exporter_component)
                            else {
                                trace!(
                                    name = import_name,
                                    "exporting component not found in all components, skipping"
                                );
                                continue;
                            };
                            let Some((ComponentItem::ComponentInstance(_), idx)) =
                                plugin_component.component.export_index(None, import_name)
                            else {
                                trace!(name = import_name, "skipping non-instance import");
                                continue;
                            };
                            (plugin_component, idx)
                        };
                        trace!(name = import_name, index = ?instance_idx, "found import at index");

                        // Preinstantiate the instance so we can use it later
                        let pre = linker.instantiate_pre(&plugin_component.component)?;

                        let mut linker_instance = match linker.instance(import_name) {
                            Ok(i) => i,
                            Err(e) => {
                                trace!(name = import_name, error = %e, "error finding instance in linker, skipping");
                                continue;
                            }
                        };

                        for (export_name, export_ty) in
                            import_instance_ty.exports(plugin_component.component.engine())
                        {
                            match export_ty {
                                ComponentItem::ComponentFunc(_func_ty) => {
                                    let (item, func_idx) = match plugin_component
                                        .component
                                        .export_index(Some(&instance_idx), export_name)
                                    {
                                        Some(res) => res,
                                        None => {
                                            trace!(
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
                                    trace!(
                                        name = import_name,
                                        fn_name = export_name,
                                        "linking function import"
                                    );
                                    let import_name: Arc<str> = import_name.into();
                                    let export_name: Arc<str> = export_name.into();
                                    let pre = pre.clone();
                                    let instance = instance.clone();
                                    linker_instance
                                        .func_new_async(
                                            &export_name.clone(),
                                            move |mut store, params, results| {
                                                // TODO: some kind of store data hashing mechanism
                                                // to detect a diff store to drop the old one
                                                let import_name = import_name.clone();
                                                let export_name = export_name.clone();
                                                let pre = pre.clone();
                                                let instance = instance.clone();
                                                Box::new(async move {
                                                    let existing_instance = instance.read().await;
                                                    let store_id = store.data().id.clone();
                                                    let instance = if let Some((id, instance)) =
                                                        existing_instance.clone()
                                                        && id == store_id
                                                    {
                                                        drop(existing_instance);
                                                        instance
                                                    } else {
                                                        // Likely unnecessary, but explicit drop of the read lock
                                                        let new_instance = pre
                                                            .instantiate_async(&mut store)
                                                            .await?;
                                                        drop(existing_instance);
                                                        *instance.write().await =
                                                            Some((store_id, new_instance));
                                                        new_instance
                                                    };

                                                    let func = instance
                                                        .get_func(&mut store, func_idx)
                                                        .context("function not found")?;
                                                    trace!(
                                                        name = %import_name,
                                                        fn_name = %export_name,
                                                        ?params,
                                                        "lowering params"
                                                    );
                                                    let mut params_buf =
                                                        Vec::with_capacity(params.len());
                                                    for v in params {
                                                        params_buf.push(
                                                            lower(&mut store, v).context(
                                                                "failed to lower parameter",
                                                            )?,
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
                                                    // TODO(IMPORTANT): Enforce a timeout on this call
                                                    // to prevent hanging indefinitely.
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
                                                    for (i, v) in
                                                        results_buf.into_iter().enumerate()
                                                    {
                                                        results[i] = lift(&mut store, v)
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
                                    let (item, _idx) = match plugin_component
                                        .component
                                        .export_index(Some(&instance_idx), export_name)
                                    {
                                        Some(res) => res,
                                        None => {
                                            trace!(
                                                name = import_name,
                                                resource = export_name,
                                                "failed to get resource index, skipping"
                                            );
                                            continue;
                                        }
                                    };
                                    let ComponentItem::Resource(_) = item else {
                                        trace!(
                                            name = import_name,
                                            resource = export_name,
                                            "expected resource export, found non-resource, skipping"
                                        );
                                        continue;
                                    };

                                    // TODO(ISSUE#4): This should get caught by the host resource check, but it isn't
                                    if export_name == "output-stream"
                                        || export_name == "input-stream"
                                        || export_name == "pollable"
                                        || export_name == "tcp-socket"
                                        || export_name == "incoming-value-async-body"
                                    {
                                        trace!(
                                            name = import_name,
                                            resource = export_name,
                                            "skipping stream link as it is a host resource type"
                                        );
                                        continue;
                                    }

                                    trace!(name = import_name, resource = export_name, ty = ?resource_ty, "linking resource import");

                                    linker_instance
                                        .resource(export_name, ResourceType::host::<ResourceAny>(), |_, _| Ok(()))
                                        .with_context(|| {
                                            format!(
                                                "failed to define resource import: {import_name}.{export_name}"
                                            )
                                        })
                                        .unwrap_or_else(|e| {
                                            trace!(name = import_name, resource = export_name, error = %e, "error defining resource import, skipping");
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
                        trace!(
                            name = import_name,
                            ty = ?resource_ty,
                            "component import is a resource, which is not supported in this context. skipping."
                        );
                    }
                    _ => continue,
                }
            }
        }
        Ok(())
    }

    /// Gets the unique identifier of the workload
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Gets the name of the workload
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Gets the namespace of the workload
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub async fn new_store(&self, component_id: &str) -> anyhow::Result<wasmtime::Store<Ctx>> {
        let components = self.components.read().await;
        let component = components
            .get(component_id)
            .context("component ID not found in workload")?;

        let mut ctx_builder = Ctx::builder().with_wasi_ctx(
            WasiCtxBuilder::new()
                .preopened_dir(
                    // TODO: actual volume mount
                    "/tmp/testy",
                    "/tmp",
                    DirPerms::all(),
                    FilePerms::all(),
                )?
                .build(),
        );

        if let Some(plugins) = &component.plugins {
            ctx_builder = ctx_builder.with_plugins(plugins.clone());
        }

        let store = wasmtime::Store::new(component.engine(), ctx_builder.build());

        Ok(store)
    }

    pub async fn instantiate_pre(
        &self,
        component_id: &str,
    ) -> anyhow::Result<wasmtime::component::InstancePre<Ctx>> {
        let mut components = self.components.write().await;
        let component = components
            .get_mut(component_id)
            .context("component ID not found in workload")?;
        let wasmtime_component = component.component.clone();
        let linker = component.linker();
        let pre = linker.instantiate_pre(&wasmtime_component)?;

        Ok(pre)
    }
}

/// An unresolved workload that has been initialized but not yet bound to plugins.
///
/// An `UnresolvedWorkload` represents a workload that has been validated and compiled
/// but has not yet been bound to host plugins or had its dependencies resolved.
/// This is an intermediate state in the workload lifecycle before becoming a
/// [`ResolvedWorkload`] that can be executed.
///
/// # Lifecycle
///
/// 1. **Creation**: Built from a [`Workload`] specification via [`Engine::initialize_workload`]
/// 2. **Plugin Binding**: Components are bound to required host plugins
/// 3. **Resolution**: Dependencies are resolved and the workload becomes [`ResolvedWorkload`]
/// 4. **Execution**: The resolved workload can create component instances and handle requests
///
/// # Plugin Resolution
///
/// During resolution, the workload will:
/// - Match required interfaces with available plugins
/// - Configure component linkers with plugin implementations
/// - Validate that all dependencies can be satisfied
/// - Create the final executable workload representation
pub struct UnresolvedWorkload {
    /// The unique identifier of the workload, created with [uuid::Uuid::new_v4]
    id: String,
    /// The name of the workload
    name: String,
    /// The namespace of the workload
    namespace: String,
    /// The requested host [`WitInterface`]s to resolve this workload
    host_interfaces: Vec<WitInterface>,
    /// The [`Service`] associated with this workload, if any
    _service: Option<Service>,
    /// All [`WorkloadComponent`]s in the workload
    components: HashMap<String, WorkloadComponent>,
}

impl UnresolvedWorkload {
    /// Creates a new unresolved workload from its constituent parts.
    ///
    /// # Arguments
    /// * `id` - Unique identifier for this workload instance
    /// * `name` - Human-readable name of the workload
    /// * `namespace` - Namespace for workload organization
    /// * `engine` - The WebAssembly engine for compilation and execution
    /// * `service` - Optional long-running service component
    /// * `components` - Iterator of components that make up this workload
    /// * `host_interfaces` - Required WIT interfaces that must be provided by host plugins
    ///
    /// # Returns
    /// A new `UnresolvedWorkload` ready for plugin binding and resolution.
    pub fn new(
        id: String,
        name: String,
        namespace: String,
        service: Option<Service>,
        components: impl IntoIterator<Item = WorkloadComponent>,
        host_interfaces: Vec<WitInterface>,
    ) -> Self {
        Self {
            id,
            name,
            namespace,
            _service: service,
            components: components
                .into_iter()
                .map(|c| (c.id().to_string(), c))
                .collect(),
            host_interfaces,
        }
    }

    /// Bind this workload to the host plugins based on the requested
    /// interfaces. Returns the list of plugins that were bound to this
    /// workload for later use.
    async fn bind_plugins(
        &mut self,
        plugins: &HashMap<&'static str, Arc<(dyn HostPlugin + 'static)>>,
    ) -> anyhow::Result<Vec<(Arc<(dyn HostPlugin + 'static)>, String)>> {
        let mut bound_plugins = Vec::new();

        // Iterate through each workload component, compare its wit world to its host interfaces,
        // and bind to plugins when a match is found.
        for (_id, workload_component) in self.components.iter_mut() {
            debug!("binding plugins for component");

            let world = workload_component.world()?;
            for wit_interface in self.host_interfaces.iter() {
                if !world.includes(wit_interface) {
                    debug!(interface = %wit_interface, "component does not use this host interface, skipping");
                    continue;
                }

                debug!(interface = %wit_interface, "checking interface for plugin binding");
                // Ensure we find a plugin for each requested `wit_interface`
                let mut found_plugin = None;
                for (id, p) in plugins.iter() {
                    let plugin_interfaces = p.world();
                    trace!(plugin_id = id, plugin_interfaces = ?plugin_interfaces, "checking plugin interfaces");

                    // Check if plugin supports this interface (ignoring config which is binding-specific)
                    let interface_match = plugin_interfaces
                        .imports
                        .iter()
                        .any(|pi| pi.contains(wit_interface))
                        || plugin_interfaces
                            .exports
                            .iter()
                            .any(|pi| pi.contains(wit_interface));
                    if interface_match {
                        trace!(id, "binding plugin to workload component");
                        if let Err(e) = p
                            .bind_component(
                                workload_component,
                                HashSet::from([wit_interface.clone()]),
                            )
                            .await
                        {
                            tracing::error!(plugin_id = id, component_id = workload_component.id(), err = ?e, "failed to bind workload to plugin");
                            bail!(e)
                        } else {
                            trace!(
                                plugin_id = id,
                                component_id = workload_component.id(),
                                "successfully bound plugin to component"
                            );
                            workload_component.add_plugin(id, p.clone());
                            found_plugin = Some((p.clone(), workload_component.id().to_string()));
                            continue;
                        }
                    }
                }

                if let Some(plugin) = found_plugin {
                    bound_plugins.push(plugin);
                } else {
                    // If no plugin matches the requested wit_interface, then the workload cannot be resolved
                    tracing::error!(interface = %wit_interface, "no plugin found for requested interface");
                    bail!(
                        "workload requested interface that is not available on this host: {wit_interface}"
                    )
                }
            }
        }

        Ok(bound_plugins)
    }

    /// Resolves the workload by binding it to host plugins and creating the final executable workload.
    ///
    /// This method performs the final resolution step that transforms an unresolved workload
    /// into a [`ResolvedWorkload`] ready for execution. It:
    ///
    /// 1. Binds components to matching host plugins based on required interfaces
    /// 2. Configures component linkers with plugin implementations
    /// 3. Validates that all component dependencies are satisfied
    /// 4. Creates the final resolved workload representation
    /// 5. Notifies plugins that the workload has been resolved
    ///
    /// # Arguments
    /// * `plugins` - Optional map of available host plugins for binding
    ///
    /// # Returns
    /// A [`ResolvedWorkload`] ready for component instantiation and execution.
    ///
    /// # Errors
    /// Returns an error if:
    /// - Required interfaces cannot be satisfied by available plugins
    /// - Plugin binding fails
    /// - Component linking fails
    /// - Plugin notification fails
    pub async fn resolve(
        mut self,
        plugins: Option<&HashMap<&'static str, Arc<dyn HostPlugin + 'static>>>,
    ) -> anyhow::Result<ResolvedWorkload> {
        // Bind to plugins
        let bound_plugins = if let Some(plugins) = plugins {
            trace!("binding plugins to workload");
            self.bind_plugins(plugins).await?
        } else {
            Vec::new()
        };

        // Resolve the workload
        let resolved_workload = ResolvedWorkload {
            id: self.id,
            name: self.name,
            namespace: self.namespace,
            components: Arc::new(RwLock::new(self.components)),
        };

        // TODO: Needs to be a set of component IDs
        // Notify plugins of the resolved workload
        for (plugin, component_id) in bound_plugins.iter() {
            trace!(
                plugin_id = plugin.id(),
                "notifying plugin of resolved workload"
            );
            plugin
                .on_workload_resolved(&resolved_workload, component_id.as_str())
                .await?;
        }

        resolved_workload.link_components().await?;

        Ok(resolved_workload)
    }
}
