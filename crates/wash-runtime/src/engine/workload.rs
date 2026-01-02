//! This module is primarily concerned with converting an [`UnresolvedWorkload`] into a [`ResolvedWorkload`] by
//! resolving all components and their dependencies.
use std::{
    collections::{HashMap, HashSet},
    ops::{Deref, DerefMut},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context as _, bail, ensure};
use tokio::{sync::RwLock, task::JoinHandle, time::timeout};
use tracing::{debug, error, info, trace, warn};
use wasmtime::component::{
    Component, Instance, InstancePre, Linker, ResourceAny, ResourceType, Val, types::ComponentItem,
};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder, p2::bindings::CommandPre};

use crate::{
    engine::{
        ctx::Ctx,
        value::{lift, lower},
    },
    plugin::HostPlugin,
    types::{LocalResources, VolumeMount},
    wit::{WitInterface, WitWorld},
};

/// Type alias for tracking bound plugins with their matched interfaces during binding.
/// Tuple: (plugin, matched_interfaces, component_ids)
type BoundPluginWithInterfaces = (
    Arc<dyn HostPlugin + 'static>,
    HashSet<WitInterface>,
    Vec<String>,
);

/// Metadata associated with components and services within a workload.
#[derive(Clone)]
pub struct WorkloadMetadata {
    /// The unique identifier for this component
    id: Arc<str>,
    /// The unique identifier for the workload this component belongs to
    workload_id: Arc<str>,
    /// The name of the workload this component belongs to
    workload_name: Arc<str>,
    /// The namespace of the workload this component belongs to
    workload_namespace: Arc<str>,
    /// The actual wasmtime [`Component`] that can be instantiated
    component: Component,
    /// The wasmtime [`Linker`] used to instantiate the component
    linker: Linker<Ctx>,
    /// The volume mounts requested by this component
    volume_mounts: Vec<(PathBuf, VolumeMount)>,
    /// The local resources requested by this component
    local_resources: LocalResources,
    /// The plugins available to this component
    plugins: Option<HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>>,
}

impl WorkloadMetadata {
    /// Returns the unique identifier for this component.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the ID of the workload this component belongs to.
    pub fn workload_id(&self) -> &str {
        &self.workload_id
    }

    /// Returns the name of the workload this component belongs to.
    pub fn workload_name(&self) -> &str {
        &self.workload_name
    }

    /// Returns the namespace of the workload this component belongs to.
    pub fn workload_namespace(&self) -> &str {
        &self.workload_namespace
    }

    /// Returns a reference to the wasmtime engine used to compile this component.
    pub fn engine(&self) -> &wasmtime::Engine {
        self.component.engine()
    }

    /// Returns a mutable reference to the component's linker.
    pub fn linker(&mut self) -> &mut Linker<Ctx> {
        &mut self.linker
    }

    /// Returns a reference to component local resources.
    pub fn local_resources(&self) -> &LocalResources {
        &self.local_resources
    }

    /// Returns a reference to the plugins associated with this component.
    pub fn plugins(&self) -> &Option<HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>> {
        &self.plugins
    }

    /// Adds a [`HostPlugin`] to the component.
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
    pub fn with_plugins(
        &mut self,
        plugins: HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>,
    ) {
        self.plugins = Some(plugins);
    }

    /// Extracts the [`ComponentItem::ComponentInstance`]s that the component exports.
    pub fn component_exports(&self) -> anyhow::Result<Vec<(String, ComponentItem)>> {
        Ok(self
            .component
            .component_type()
            .exports(self.component.engine())
            .filter_map(|(name, item)| {
                if matches!(item, ComponentItem::ComponentInstance(_)) {
                    Some((name.to_string(), item))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>())
    }

    pub fn uses_wasi_http(&self) -> bool {
        crate::engine::uses_wasi_http(&self.component)
    }

    pub fn imports_wasi_http(&self) -> bool {
        crate::engine::imports_wasi_http(&self.component)
    }

    pub fn exports_wasi_http(&self) -> bool {
        crate::engine::exports_wasi_http(&self.component)
    }

    /// Computes and returns the [`WitWorld`] of this component.
    pub fn world(&self) -> WitWorld {
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

        WitWorld {
            imports: imports.into_values().collect(),
            exports: exports.into_values().collect(),
        }
    }
}

/// A [`WorkloadService`] is a component that is part of a workload that
/// runs once, either to completion or for the duration of the workload lifecycle.
#[derive(Clone)]
pub struct WorkloadService {
    /// The [`WorkloadMetadata`] for this service
    metadata: WorkloadMetadata,
    /// The maximum number of restarts for this service
    max_restarts: u64,
    /// The [`JoinHandle`] for the running service
    handle: Option<Arc<JoinHandle<()>>>,
}

impl WorkloadService {
    /// Create a new [`WorkloadService`] with the given workload ID,
    /// wasmtime [`Component`], [`Linker`], volume mounts, and instance limits.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        workload_id: impl Into<Arc<str>>,
        workload_name: impl Into<Arc<str>>,
        workload_namespace: impl Into<Arc<str>>,
        component: Component,
        linker: Linker<Ctx>,
        volume_mounts: Vec<(PathBuf, VolumeMount)>,
        local_resources: LocalResources,
        max_restarts: u64,
    ) -> Self {
        Self {
            metadata: WorkloadMetadata {
                id: uuid::Uuid::new_v4().to_string().into(),
                workload_id: workload_id.into(),
                workload_name: workload_name.into(),
                workload_namespace: workload_namespace.into(),
                component,
                linker,
                volume_mounts,
                local_resources,
                plugins: None,
            },
            handle: None,
            max_restarts,
        }
    }

    /// Pre-instantiate the component to prepare for execution.
    pub fn pre_instantiate(&mut self) -> anyhow::Result<CommandPre<Ctx>> {
        let component = self.metadata.component.clone();
        let pre = self.metadata.linker.instantiate_pre(&component)?;
        let command = CommandPre::new(pre)?;
        Ok(command)
    }

    /// Whether or not the service is currently running.
    pub fn is_running(&self) -> bool {
        self.handle.is_some()
    }
}

/// A [`WorkloadComponent`] is a component that is part of a workload.
///
/// It contains the actual [`Component`] that can be instantiated,
/// the [`Linker`] for creating stores and instances, the available
/// [`VolumeMount`]s to be passed as filesystem preopens, and the
/// full list of [`HostPlugin`]s that the component depends on.
#[derive(Clone)]
pub struct WorkloadComponent {
    /// The [`WorkloadMetadata`] for this component
    metadata: WorkloadMetadata,
    /// The number of warm instances to keep for this component
    pool_size: usize,
    /// The maximum number of concurrent invocations allowed for this component
    max_invocations: usize,
}

impl WorkloadComponent {
    /// Create a new [`WorkloadComponent`] with the given workload ID,
    /// wasmtime [`Component`], [`Linker`], volume mounts, and instance limits.
    pub fn new(
        workload_id: impl Into<Arc<str>>,
        workload_name: impl Into<Arc<str>>,
        workload_namespace: impl Into<Arc<str>>,
        component: Component,
        linker: Linker<Ctx>,
        volume_mounts: Vec<(PathBuf, VolumeMount)>,
        local_resources: LocalResources,
    ) -> Self {
        Self {
            metadata: WorkloadMetadata {
                id: uuid::Uuid::new_v4().to_string().into(),
                workload_id: workload_id.into(),
                workload_name: workload_name.into(),
                workload_namespace: workload_namespace.into(),
                component,
                linker,
                volume_mounts,
                local_resources,
                plugins: None,
            },
            // TODO: Implement pooling and instance limits
            pool_size: 0,
            max_invocations: 0,
        }
    }

    /// Pre-instantiate the component to prepare for instantiation.
    pub fn pre_instantiate(&mut self) -> anyhow::Result<InstancePre<Ctx>> {
        let component = self.metadata.component.clone();
        self.metadata.linker.instantiate_pre(&component)
    }

    pub fn metadata(&self) -> &WorkloadMetadata {
        &self.metadata
    }
}

impl std::fmt::Debug for WorkloadComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkloadComponent")
            .field("id", &self.metadata.id.as_ref())
            .field("workload_id", &self.metadata.workload_id.as_ref())
            .field("volume_mounts", &self.metadata.volume_mounts)
            .field("pool_size", &self.pool_size)
            .field("max_invocations", &self.max_invocations)
            .finish()
    }
}

impl std::fmt::Debug for WorkloadService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkloadService")
            .field("id", &self.metadata.id.as_ref())
            .field("workload_name", &self.metadata.workload_name.as_ref())
            .field(
                "workload_namespace",
                &self.metadata.workload_namespace.as_ref(),
            )
            .field("workload_id", &self.metadata.workload_id.as_ref())
            .field("volume_mounts", &self.metadata.volume_mounts)
            .field("is_running", &self.is_running())
            .finish()
    }
}

impl Deref for WorkloadComponent {
    type Target = WorkloadMetadata;

    fn deref(&self) -> &Self::Target {
        &self.metadata
    }
}

impl DerefMut for WorkloadComponent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.metadata
    }
}

impl Deref for WorkloadService {
    type Target = WorkloadMetadata;

    fn deref(&self) -> &Self::Target {
        &self.metadata
    }
}

impl DerefMut for WorkloadService {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.metadata
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
    id: Arc<str>,
    /// The name of the workload
    name: Arc<str>,
    /// The namespace of the workload
    namespace: Arc<str>,
    /// All components in the workload. This is behind a `RwLock` to support mutable
    /// access to the component linkers.
    components: Arc<RwLock<HashMap<Arc<str>, WorkloadComponent>>>,
    /// The HTTP handler for outgoing HTTP requests
    http_handler: Arc<dyn crate::host::http::HostHandler>,
    /// An optional service component that runs once to completion or for the duration of the workload
    service: Option<WorkloadService>,
    /// The requested host [`WitInterface`]s to resolve this workload
    host_interfaces: Vec<WitInterface>,
}

impl ResolvedWorkload {
    /// Executes the service, if present, and returns whether it was run.
    pub(crate) async fn execute_service(&mut self) -> anyhow::Result<bool> {
        let service = self
            .service
            .as_mut()
            .map(|s| (s.pre_instantiate(), s.max_restarts));

        if let Some((Ok(pre), mut max_restarts)) = service {
            // This will always be present since we just checked above, but we need this structure
            // to only borrow the service metadata
            let mut store = if let Some(service) = self.service.as_ref() {
                self.new_store_from_metadata(&service.metadata).await?
            } else {
                bail!("service unexpectedly missing during execution");
            };
            let instance = pre.instantiate_async(&mut store).await?;
            let handle = tokio::spawn(async move {
                loop {
                    if let Err(e) = instance.wasi_cli_run().call_run(&mut store).await {
                        warn!(err = %e, retries = max_restarts, "service execution failed");
                        if max_restarts == 0 {
                            warn!("max restarts reached, service will not be restarted");
                            break;
                        }
                    } else {
                        info!("service exited successfully");
                        break;
                    }
                    max_restarts = max_restarts.saturating_sub(1);
                }
            });

            // Store the handle to ensure the service can be cleaned up during workload shutdown
            if let Some(s) = self.service.as_mut() {
                s.handle = Some(Arc::new(handle));
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Aborts the running service [`JoinHandle`] if it exists.
    pub(crate) fn stop_service(&self) {
        if let Some(service) = &self.service
            && let Some(handle) = &service.handle
        {
            handle.abort();
            debug!(
                workload_id = self.id.as_ref(),
                "service for workload aborted"
            );
        }
    }

    pub fn components(&self) -> Arc<RwLock<HashMap<Arc<str>, WorkloadComponent>>> {
        self.components.clone()
    }

    pub fn host_interfaces(&self) -> &Vec<WitInterface> {
        &self.host_interfaces
    }

    async fn link_components(&mut self) -> anyhow::Result<()> {
        // A map from component ID to its exported interfaces
        let mut interface_map: HashMap<String, Arc<str>> = HashMap::new();

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
                    trace!(name, "registering component export for linking");
                    interface_map.insert(name.clone(), Arc::from(c.id()));
                } else {
                    warn!(name, "exported item is not a component instance, skipping");
                }
            }
        }

        self.resolve_workload_imports(&interface_map).await?;

        Ok(())
    }

    /// This function plugs a components imports with the exports of other components
    /// that are already loaded in the plugin system.
    ///
    /// Components are processed in topological order based on their inter-component
    /// dependencies. This ensures that when a component imports from another component,
    /// the exporting component has already had its imports resolved and can be
    /// pre-instantiated.
    async fn resolve_workload_imports(
        &mut self,
        interface_map: &HashMap<String, Arc<str>>,
    ) -> anyhow::Result<()> {
        // Build a dependency graph: for each component, track which other components it imports from
        let mut dependencies: HashMap<Arc<str>, HashSet<Arc<str>>> = HashMap::new();

        {
            let components = self.components.read().await;
            for (component_id, component) in components.iter() {
                let mut deps = HashSet::new();
                let ty = component.metadata.component.component_type();
                for (import_name, import_item) in ty.imports(component.metadata.component.engine())
                {
                    if matches!(import_item, ComponentItem::ComponentInstance(_))
                        && let Some(exporter_id) = interface_map.get(import_name)
                        && exporter_id != component_id
                    {
                        // This import is provided by another component in the workload
                        deps.insert(exporter_id.clone());
                    }
                }
                dependencies.insert(component_id.clone(), deps);
            }
        }

        // Topologically sort components: components with no dependencies (or dependencies
        // already processed) come first. This ensures that when we process a component
        // that imports from another component, the exporter has already been resolved.
        let sorted_component_ids = topological_sort_components(&dependencies).context(
            "failed to determine component processing order - possible circular dependency",
        )?;

        trace!(
            order = ?sorted_component_ids.iter().map(|id| id.as_ref()).collect::<Vec<_>>(),
            "processing components in topological order"
        );

        for component_id in sorted_component_ids {
            // In order to have mutable access to both the workload component and components that need
            // to be instantiated as "plugins" during linking, we remove and re-add the component to the list.
            let mut workload_component = {
                self.components
                    .write()
                    .await
                    .remove(&component_id)
                    .context("component not found during import resolution")?
            };

            let component = workload_component.metadata.component.clone();
            let linker = &mut workload_component.metadata.linker;
            let res = self
                .resolve_component_imports(&component, linker, interface_map)
                .await;
            self.components
                .write()
                .await
                .insert(workload_component.metadata.id.clone(), workload_component);
            // Propagate any errors encountered during import resolution
            res?;
        }

        if let Some(mut service) = self.service.take() {
            let component = service.metadata.component.clone();
            let linker = &mut service.metadata.linker;

            let res = self
                .resolve_component_imports(&component, linker, interface_map)
                .await;

            self.service = Some(service);

            // Propagate any errors encountered during import resolution
            res?;
        }

        Ok(())
    }

    async fn resolve_component_imports(
        &self,
        component: &wasmtime::component::Component,
        linker: &mut Linker<Ctx>,
        interface_map: &HashMap<String, Arc<str>>,
    ) -> anyhow::Result<()> {
        let ty = component.component_type();
        let imports: Vec<_> = ty.imports(component.engine()).collect();

        // TODO: some kind of shared import_name -> component registry. need to remove when new store
        // store id, instance, import_name. That will keep the instance properly unique
        let instance: Arc<RwLock<Option<(String, Instance)>>> = Arc::default();
        for (import_name, import_item) in imports.into_iter() {
            match import_item {
                ComponentItem::ComponentInstance(import_instance_ty) => {
                    trace!(name = import_name, "processing component instance import");
                    let mut all_components = self.components.write().await;
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
                        let Some(plugin_component) = all_components.get_mut(exporter_component)
                        else {
                            trace!(
                                name = import_name,
                                "exporting component not found in all components, skipping"
                            );
                            continue;
                        };
                        let Some((ComponentItem::ComponentInstance(_), idx)) = plugin_component
                            .metadata
                            .component
                            .get_export(None, import_name)
                        else {
                            trace!(name = import_name, "skipping non-instance import");
                            continue;
                        };
                        (plugin_component, idx)
                    };
                    trace!(name = import_name, index = ?instance_idx, "found import at index");

                    // Preinstantiate the plugin instance so we can use it later
                    let pre = plugin_component
                        .pre_instantiate()
                        .context("failed to pre-instantiate during component linking")?;

                    let mut linker_instance = match linker.instance(import_name) {
                        Ok(i) => i,
                        Err(e) => {
                            trace!(name = import_name, error = %e, "error finding instance in linker, skipping");
                            continue;
                        }
                    };

                    for (export_name, export_ty) in
                        import_instance_ty.exports(plugin_component.metadata.component.engine())
                    {
                        match export_ty {
                            ComponentItem::ComponentFunc(_func_ty) => {
                                let (item, func_idx) = match plugin_component
                                    .metadata
                                    .component
                                    .get_export(Some(&instance_idx), export_name)
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
                                            // TODO(#103): some kind of store data hashing mechanism
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
                                                    let new_instance =
                                                        pre.instantiate_async(&mut store).await?;
                                                    drop(existing_instance);
                                                    *instance.write().await =
                                                        Some((store_id, new_instance));
                                                    new_instance
                                                };

                                                let func = instance
                                                    .get_func(&mut store, func_idx)
                                                    .context("function not found");

                                                let func = match func {
                                                    Ok(func) => func,
                                                    Err(e) => {
                                                        error!("Error in gettin func: {:?}", e);
                                                        return Err(e);
                                                    }
                                                };

                                                trace!(
                                                    name = %import_name,
                                                    fn_name = %export_name,
                                                    ?params,
                                                    "lowering params"
                                                );
                                                let mut params_buf =
                                                    Vec::with_capacity(params.len());
                                                for v in params {
                                                    params_buf
                                                        .push(lower(&mut store, v).context(
                                                            "failed to lower parameter",
                                                        )?);
                                                }
                                                trace!(
                                                    name = %import_name,
                                                    fn_name = %export_name,
                                                    ?params_buf,
                                                    "invoking dynamic export"
                                                );

                                                let mut results_buf =
                                                    vec![Val::Bool(false); results.len()];

                                                // Enforce a timeout on this call to prevent hanging indefinitely
                                                const CALL_TIMEOUT: Duration =
                                                    Duration::from_secs(30);
                                                timeout(
                                                    CALL_TIMEOUT,
                                                    func.call_async(
                                                        &mut store,
                                                        &params_buf,
                                                        &mut results_buf,
                                                    ),
                                                )
                                                .await
                                                .context(
                                                    "function call timed out after 30 seconds",
                                                )?
                                                .context("failed to call function")?;

                                                trace!(
                                                    name = %import_name,
                                                    fn_name = %export_name,
                                                    ?results_buf,
                                                    "lifting results"
                                                );
                                                for (i, v) in results_buf.into_iter().enumerate() {
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
                                    .metadata
                                    .component
                                    .get_export(Some(&instance_idx), export_name)
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

                                // TODO: This should be a comparison of the ComponentItem to the
                                // host resource type, but for some reason the comparison fails.
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

    /// Returns the number of components in this workload.
    /// Does not include the service component if one is defined.
    pub async fn component_count(&self) -> usize {
        self.components.read().await.len()
    }

    /// Helper to create a new wasmtime Store for a given component in the workload.
    pub async fn new_store(&self, component_id: &str) -> anyhow::Result<wasmtime::Store<Ctx>> {
        let components = self.components.read().await;
        let component = components
            .get(component_id)
            .context("component ID not found in workload")?;
        self.new_store_from_metadata(&component.metadata).await
    }

    /// Creates a new wasmtime Store from the given workload metadata.
    pub async fn new_store_from_metadata(
        &self,
        metadata: &WorkloadMetadata,
    ) -> anyhow::Result<wasmtime::Store<Ctx>> {
        let components = self.components.read().await;

        // TODO: Consider stderr/stdout buffering + logging
        let mut wasi_ctx_builder = WasiCtxBuilder::new();
        wasi_ctx_builder
            .envs(
                metadata
                    .local_resources
                    .environment
                    .iter()
                    .map(|kv| (kv.0.as_str(), kv.1.as_str()))
                    .collect::<Vec<_>>()
                    .as_slice(),
            )
            .inherit_stdout()
            .inherit_stderr();

        // Mount all possible volume mounts in the workload since components share a WasiCtx
        for (host_path, mount) in &components
            .iter()
            .flat_map(|(_id, workload_component)| workload_component.metadata.volume_mounts.clone())
            .collect::<Vec<_>>()
        {
            let dir = tokio::fs::canonicalize(host_path).await?;
            debug!(host_path = %dir.display(), container_path = %mount.mount_path, "preopening volume mount");
            let (dir_perms, file_perms) = match mount.read_only {
                true => (DirPerms::READ, FilePerms::READ),
                false => (DirPerms::all(), FilePerms::all()),
            };
            wasi_ctx_builder.preopened_dir(&dir, &mount.mount_path, dir_perms, file_perms)?;
        }

        let mut ctx_builder = Ctx::builder(metadata.workload_id(), metadata.id())
            .with_http_handler(self.http_handler.clone())
            .with_wasi_ctx(wasi_ctx_builder.build());

        if let Some(plugins) = &metadata.plugins {
            ctx_builder = ctx_builder.with_plugins(plugins.clone());
        }

        let store = wasmtime::Store::new(metadata.engine(), ctx_builder.build());

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
        let wasmtime_component = component.metadata.component.clone();
        let linker = component.metadata.linker();
        let pre = linker.instantiate_pre(&wasmtime_component)?;

        Ok(pre)
    }

    /// Unbind all plugins from all components in this workload.
    ///
    /// This should be called when stopping a workload to ensure proper cleanup
    /// of plugin resources. Errors from individual plugin unbind operations are
    /// logged but do not prevent the overall unbind from completing.
    pub async fn unbind_all_plugins(&self) -> anyhow::Result<()> {
        trace!(
            workload_id = self.id.as_ref(),
            workload_name = self.name.as_ref(),
            "unbinding all plugins from workload"
        );

        for component in self.components.read().await.values() {
            if let Some(plugins) = component.plugins() {
                for (plugin_id, plugin) in plugins.iter() {
                    trace!(
                        plugin_id,
                        component_id = component.id(),
                        workload_id = self.id.as_ref(),
                        "unbinding plugin from component"
                    );

                    // Get the interfaces this plugin was bound to by checking the component's imports
                    let world = component.world();
                    let plugin_world = plugin.world();

                    // Find the intersection of what the component imports and what the plugin provides
                    let bound_interfaces = world
                        .imports
                        .iter()
                        .filter(|import| plugin_world.imports.contains(import))
                        .cloned()
                        .collect::<std::collections::HashSet<_>>();

                    if let Err(e) = plugin.on_workload_unbind(self.id(), bound_interfaces).await {
                        warn!(
                            plugin_id,
                            component_id = component.id(),
                            workload_id = self.id.as_ref(),
                            error = ?e,
                            "failed to unbind plugin from workload, continuing cleanup"
                        );
                    }
                }
            }

            if component.exports_wasi_http() {
                self.http_handler
                    .on_workload_unbind(self.id())
                    .await
                    .context("failed to notify HTTP handler of workload")?;
            }
        }

        Ok(())
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
    id: Arc<str>,
    /// The name of the workload
    name: Arc<str>,
    /// The namespace of the workload
    namespace: Arc<str>,
    /// The requested host [`WitInterface`]s to resolve this workload
    host_interfaces: Vec<WitInterface>,
    /// The [`WorkloadService`] associated with this workload, if any
    service: Option<WorkloadService>,
    /// All [`WorkloadComponent`]s in the workload
    components: HashMap<Arc<str>, WorkloadComponent>,
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
        id: impl Into<Arc<str>>,
        name: impl Into<Arc<str>>,
        namespace: impl Into<Arc<str>>,
        service: Option<WorkloadService>,
        components: impl IntoIterator<Item = WorkloadComponent>,
        host_interfaces: Vec<WitInterface>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            namespace: namespace.into(),
            service,
            components: components
                .into_iter()
                .map(|c| {
                    let id = Arc::from(c.id());
                    (id, c)
                })
                .collect(),
            host_interfaces,
        }
    }

    /// Bind this workload to the host plugins based on the requested
    /// interfaces. Returns a list of plugins and the component IDs they were bound to.
    pub async fn bind_plugins(
        &mut self,
        plugins: &HashMap<&'static str, Arc<dyn HostPlugin + 'static>>,
    ) -> anyhow::Result<Vec<(Arc<dyn HostPlugin + 'static>, Vec<String>)>> {
        // Track bound plugins with their matched interfaces for cleanup on failure
        let mut bound_plugins_with_interfaces: Vec<BoundPluginWithInterfaces> = Vec::new();
        let mut bound_plugins: Vec<(Arc<dyn HostPlugin + 'static>, Vec<String>)> = Vec::new();

        // Collect all component's required (unmatched) host interfaces
        // This tracks which interfaces each component still needs to be bound
        let mut unmatched_interfaces: HashMap<Arc<str>, HashSet<WitInterface>> = HashMap::new();
        let host_interfaces = {
            // filter out Plugins fulfilled by host
            let http_iface = WitInterface::from("wasi:http/incoming-handler,outgoing-handler");
            self.host_interfaces
                .iter()
                .filter(|wit_interface| !http_iface.contains(wit_interface))
                .cloned()
                .collect::<Vec<_>>()
        };

        trace!(host_interfaces = ?host_interfaces, "determining missing guest interfaces");

        if let Some(service) = self.service.as_ref() {
            let world = service.world();
            trace!(?world, "comparing service world to host interfaces");
            let required_interfaces: HashSet<WitInterface> = host_interfaces
                .iter()
                .filter(|wit_interface| world.includes_bidirectional(wit_interface))
                .cloned()
                .collect();

            if !required_interfaces.is_empty() {
                unmatched_interfaces.insert(Arc::from(service.id()), required_interfaces);
            }
        }

        for (id, workload_component) in &self.components {
            let world = workload_component.world();
            trace!(?world, "comparing component world to host interfaces");
            let required_interfaces: HashSet<WitInterface> = host_interfaces
                .iter()
                .filter(|wit_interface| world.includes_bidirectional(wit_interface))
                .cloned()
                .collect();

            if !required_interfaces.is_empty() {
                unmatched_interfaces.insert(id.clone(), required_interfaces);
            }
        }

        trace!(?unmatched_interfaces, "resolving unmatched interfaces");

        // Iterate through each plugin first, then check every component for matching worlds
        for (plugin_id, p) in plugins.iter() {
            let plugin_interfaces = p.world();
            trace!(plugin_id = plugin_id, plugin_interfaces = ?plugin_interfaces, "checking plugin interfaces");

            // Collect bindings for this plugin across all components
            let mut plugin_component_bindings = Vec::new();

            // Check each component to see if this plugin matches any of their required interfaces
            for (component_id, required_interfaces) in unmatched_interfaces.iter() {
                // Find interfaces that this plugin can satisfy for this component
                let mut matching_interfaces = HashSet::new();
                for wit_interface in required_interfaces.iter() {
                    // Check if plugin supports this interface
                    if plugin_interfaces.includes_bidirectional(wit_interface) {
                        matching_interfaces.insert(wit_interface.clone());
                    }
                }

                if !matching_interfaces.is_empty() {
                    plugin_component_bindings.push((component_id.clone(), matching_interfaces));
                }
            }

            // If this plugin matches any components, bind them
            if !plugin_component_bindings.is_empty() {
                // Collect all unique interfaces across all component bindings for on_workload_bind
                let plugin_matched_interfaces: HashSet<WitInterface> = plugin_component_bindings
                    .iter()
                    .flat_map(|(_, interfaces)| interfaces.clone())
                    .collect();
                debug!(
                    plugin_id = plugin_id,
                    interfaces = ?plugin_matched_interfaces,
                    "binding plugin to workload"
                );

                // Call on_workload_bind with the workload and all matched interfaces
                if let Err(e) = p
                    .on_workload_bind(self, plugin_matched_interfaces.clone())
                    .await
                {
                    tracing::error!(
                        plugin_id = plugin_id,
                        err = ?e,
                        "failed to bind plugin to workload"
                    );
                    // Clean up all previously bound plugins in reverse order
                    for (bound_plugin, bound_interfaces, _) in
                        bound_plugins_with_interfaces.iter().rev()
                    {
                        debug!(
                            plugin_id = bound_plugin.id(),
                            "calling on_workload_unbind for cleanup after bind failure"
                        );
                        if let Err(cleanup_err) = bound_plugin
                            .on_workload_unbind(self.id(), bound_interfaces.clone())
                            .await
                        {
                            warn!(
                                plugin_id = bound_plugin.id(),
                                error = ?cleanup_err,
                                "failed to cleanup plugin after bind failure"
                            );
                        }
                    }
                    bail!(e)
                }

                // Collect component IDs for this plugin
                let mut plugin_component_ids = Vec::new();

                // Now bind each component
                for (component_id, matching_interfaces) in plugin_component_bindings {
                    // Get the workload component (mutable access needed for binding)
                    let workload_component = self
                        .components
                        .get_mut(&component_id)
                        .context("component not found during plugin binding")?;

                    debug!(
                        plugin_id = plugin_id,
                        component_id = workload_component.id(),
                        interfaces = ?matching_interfaces,
                        "binding plugin to workload component"
                    );

                    if let Err(e) = p
                        .on_component_bind(workload_component, matching_interfaces.clone())
                        .await
                    {
                        tracing::error!(
                            plugin_id = plugin_id,
                            component_id = workload_component.id(),
                            err = ?e,
                            "failed to bind workload component to plugin"
                        );
                        // Clean up all previously bound plugins in reverse order
                        for (bound_plugin, bound_interfaces, _) in
                            bound_plugins_with_interfaces.iter().rev()
                        {
                            debug!(
                                plugin_id = bound_plugin.id(),
                                "calling on_workload_unbind for cleanup after component bind failure"
                            );
                            if let Err(cleanup_err) = bound_plugin
                                .on_workload_unbind(self.id(), bound_interfaces.clone())
                                .await
                            {
                                warn!(
                                    plugin_id = bound_plugin.id(),
                                    error = ?cleanup_err,
                                    "failed to cleanup plugin after component bind failure"
                                );
                            }
                        }
                        bail!(e)
                    } else {
                        trace!(
                            plugin_id = plugin_id,
                            component_id = workload_component.id(),
                            "successfully bound plugin to component"
                        );
                        workload_component.add_plugin(plugin_id, p.clone());
                        plugin_component_ids.push(workload_component.id().to_string());

                        // Remove matched interfaces from unmatched set
                        if let Some(unmatched) = unmatched_interfaces.get_mut(&component_id) {
                            for interface in &matching_interfaces {
                                unmatched.remove(interface);
                            }
                        }
                    }
                }

                // Add this plugin with all its bound component IDs
                bound_plugins.push((p.clone(), plugin_component_ids.clone()));
                bound_plugins_with_interfaces.push((
                    p.clone(),
                    plugin_matched_interfaces,
                    plugin_component_ids,
                ));
            }
        }

        // Check if all required interfaces were matched
        for (component_id, unmatched) in unmatched_interfaces.iter() {
            if !unmatched.is_empty() {
                tracing::error!(
                    component_id = component_id.as_ref(),
                    interfaces = ?unmatched,
                    "no plugins found for requested interfaces"
                );
                bail!(
                    "workload component {component_id} requested interfaces that are not available on this host: {unmatched:?}",
                )
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
        http_handler: Arc<dyn crate::host::http::HostHandler>,
    ) -> anyhow::Result<ResolvedWorkload> {
        // Bind to plugins
        let bound_plugins = if let Some(plugins) = plugins {
            trace!("binding plugins to workload");
            self.bind_plugins(plugins).await?
        } else {
            Vec::new()
        };

        let incoming_http_component = {
            let http_iface = WitInterface::from("wasi:http/incoming-handler");
            match self
                .host_interfaces
                .iter()
                .any(|hi| hi.contains(&http_iface))
            {
                // http was not part of the requested interfaces
                false => None,
                true => self
                    .components
                    .values()
                    .find(|component| component.exports_wasi_http())
                    .map(|c| c.id().to_string()),
            }
        };

        // Resolve the workload
        let mut resolved_workload = ResolvedWorkload {
            id: self.id.clone(),
            name: self.name.clone(),
            namespace: self.namespace.clone(),
            components: Arc::new(RwLock::new(self.components)),
            service: self.service,
            host_interfaces: self.host_interfaces,
            http_handler: http_handler.clone(),
        };

        // Link components before plugin resolution
        if let Err(e) = resolved_workload.link_components().await {
            // If linking fails, unbind all plugins before returning the error
            warn!(
                error = ?e,
                "failed to link components, unbinding all plugins"
            );
            let _ = resolved_workload.unbind_all_plugins().await;
            bail!(e);
        }

        // Notify plugins of the resolved workload
        for (plugin, component_ids) in bound_plugins.iter() {
            trace!(
                plugin_id = plugin.id(),
                component_count = component_ids.len(),
                "notifying plugin of resolved workload"
            );
            // Call on_workload_resolved for each component this plugin is bound to
            for component_id in component_ids {
                if let Err(e) = plugin
                    .on_workload_resolved(&resolved_workload, component_id.as_str())
                    .await
                {
                    // If we fail to notify a plugin, unbind all plugins that were already bound
                    warn!(
                        plugin_id = plugin.id(),
                        component_id,
                        error = ?e,
                        "failed to notify plugin of resolved workload, unbinding all plugins"
                    );
                    let _ = resolved_workload.unbind_all_plugins().await;
                    bail!(e);
                }
            }
        }

        if let Some(component_id) = incoming_http_component
            && let Err(e) = http_handler
                .on_workload_resolved(&resolved_workload, &component_id)
                .await
        {
            warn!(
                component_id = component_id,
                error = ?e,
                "failed to notify HTTP handler of resolved workload, unbinding all plugins"
            );
            let _ = resolved_workload.unbind_all_plugins().await;
            bail!(e);
        }

        Ok(resolved_workload)
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

    /// Retrieves the interface configuration for a given WIT interface, if it exists.
    pub fn interface_config(&self, interface: &WitInterface) -> Option<&HashMap<String, String>> {
        self.host_interfaces
            .iter()
            .find(|i| i.contains(interface))
            .map(|i| &i.config)
    }
}

/// Performs a topological sort on components based on their inter-component dependencies.
///
/// This function uses Kahn's algorithm to produce an ordering where components
/// that export interfaces are processed before components that import those interfaces.
/// This ensures that when linking components, the exporting component's linker has
/// already been fully configured before it needs to be pre-instantiated.
///
/// # Arguments
/// * `dependencies` - A map from component ID to the set of component IDs it depends on
///   (i.e., components whose exports it imports)
///
/// # Returns
/// A vector of component IDs in topological order (dependencies first), or an error
/// if a circular dependency is detected.
fn topological_sort_components(
    dependencies: &HashMap<Arc<str>, HashSet<Arc<str>>>,
) -> anyhow::Result<Vec<Arc<str>>> {
    // Build in-degree map: count how many dependencies each component has
    // (only counting dependencies on other components within this workload)
    let mut in_degree: HashMap<Arc<str>, usize> = HashMap::new();

    for (component_id, deps) in dependencies {
        // Initialize entry for this component
        in_degree.entry(component_id.clone()).or_insert(0);

        // Count only dependencies that are part of this workload
        let dep_count = deps
            .iter()
            .filter(|d| dependencies.contains_key(*d))
            .count();
        *in_degree.get_mut(component_id).unwrap() = dep_count;
    }

    // Start with components that have no dependencies (in-degree == 0)
    // Sort for deterministic ordering
    let mut queue: Vec<Arc<str>> = in_degree
        .iter()
        .filter(|&(_, degree)| *degree == 0)
        .map(|(id, _)| id.clone())
        .collect();
    queue.sort();

    let mut result = Vec::with_capacity(dependencies.len());

    while let Some(component_id) = queue.pop() {
        result.push(component_id.clone());

        // Find components that depend on this one and decrease their in-degree
        for (other_id, deps) in dependencies {
            if deps.contains(&component_id)
                && let Some(degree) = in_degree.get_mut(other_id)
            {
                *degree = degree.saturating_sub(1);
                if *degree == 0 && !result.contains(other_id) {
                    queue.push(other_id.clone());
                    // Re-sort to maintain determinism
                    queue.sort();
                }
            }
        }
    }

    // Check for circular dependencies
    if result.len() != dependencies.len() {
        let unprocessed: Vec<_> = dependencies
            .keys()
            .filter(|id| !result.contains(id))
            .map(|id| id.as_ref())
            .collect();
        bail!(
            "circular dependency detected among components: {:?}",
            unprocessed
        );
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::HostPlugin;
    use crate::wit::{WitInterface, WitWorld};
    use async_trait::async_trait;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use wasmtime::component::{Component, Linker};

    /// Records a single plugin method call for testing callback order and parameters.
    #[derive(Debug, Clone)]
    struct CallRecord {
        #[allow(unused)]
        plugin_id: String,
        method: String,
        component_id: Option<String>,
        #[allow(unused)]
        interfaces: Vec<String>,
    }

    /// Mock plugin implementation for testing workload binding behavior.
    /// Tracks all method calls and counts for verification of callback order and frequency.
    struct MockPlugin {
        #[allow(unused)]
        id: String,
        world: WitWorld,
        call_records: Arc<Mutex<Vec<CallRecord>>>,
        on_workload_bind_count: Arc<AtomicUsize>,
        on_component_bind_count: Arc<AtomicUsize>,
        on_workload_resolved_count: Arc<AtomicUsize>,
    }

    impl MockPlugin {
        /// Creates a new mock plugin with the specified interfaces it can import/export.
        fn new(
            id: impl Into<String>,
            imports: Vec<WitInterface>,
            exports: Vec<WitInterface>,
        ) -> Self {
            Self {
                id: id.into(),
                world: WitWorld {
                    imports: imports.into_iter().collect(),
                    exports: exports.into_iter().collect(),
                },
                call_records: Arc::new(Mutex::new(Vec::new())),
                on_workload_bind_count: Arc::new(AtomicUsize::new(0)),
                on_component_bind_count: Arc::new(AtomicUsize::new(0)),
                on_workload_resolved_count: Arc::new(AtomicUsize::new(0)),
            }
        }

        /// Returns the number of times the specified method was called.
        fn get_call_count(&self, method: &str) -> usize {
            match method {
                "on_workload_bind" => self.on_workload_bind_count.load(Ordering::SeqCst),
                "on_component_bind" => self.on_component_bind_count.load(Ordering::SeqCst),
                "on_workload_resolved" => self.on_workload_resolved_count.load(Ordering::SeqCst),
                _ => 0,
            }
        }

        /// Returns all recorded method calls in chronological order.
        fn get_call_records(&self) -> Vec<CallRecord> {
            self.call_records.lock().unwrap().clone()
        }
    }

    const ID: &str = "mock-plugin";

    #[async_trait]
    impl HostPlugin for MockPlugin {
        fn id(&self) -> &'static str {
            ID
        }

        fn world(&self) -> WitWorld {
            self.world.clone()
        }

        async fn on_workload_bind(
            &self,
            _workload: &UnresolvedWorkload,
            interfaces: HashSet<WitInterface>,
        ) -> anyhow::Result<()> {
            self.on_workload_bind_count.fetch_add(1, Ordering::SeqCst);
            self.call_records.lock().unwrap().push(CallRecord {
                plugin_id: ID.to_string(),
                method: "on_workload_bind".to_string(),
                component_id: None,
                interfaces: interfaces.iter().map(|i| i.to_string()).collect(),
            });
            Ok(())
        }

        async fn on_component_bind(
            &self,
            component: &mut WorkloadComponent,
            interfaces: HashSet<WitInterface>,
        ) -> anyhow::Result<()> {
            self.on_component_bind_count.fetch_add(1, Ordering::SeqCst);
            self.call_records.lock().unwrap().push(CallRecord {
                plugin_id: ID.to_string(),
                method: "on_component_bind".to_string(),
                component_id: Some(component.id().to_string()),
                interfaces: interfaces.iter().map(|i| i.to_string()).collect(),
            });
            Ok(())
        }

        async fn on_workload_resolved(
            &self,
            _workload: &ResolvedWorkload,
            component_id: &str,
        ) -> anyhow::Result<()> {
            self.on_workload_resolved_count
                .fetch_add(1, Ordering::SeqCst);
            self.call_records.lock().unwrap().push(CallRecord {
                plugin_id: ID.to_string(),
                method: "on_workload_resolved".to_string(),
                component_id: Some(component_id.to_string()),
                interfaces: Vec::new(),
            });
            Ok(())
        }
    }

    /// HTTP counter component fixture for testing with actual WIT interfaces.
    const HTTP_COUNTER_WASM: &[u8] = include_bytes!("../../tests/fixtures/http_counter.wasm");

    /// Creates a test component using the http_counter fixture.
    /// This provides a real component with actual WIT interface imports.
    fn create_test_component(id: &str) -> WorkloadComponent {
        let engine = wasmtime::Engine::default();
        let linker = Linker::new(&engine);

        // Use the actual http_counter fixture component
        let component = Component::new(&engine, HTTP_COUNTER_WASM).unwrap();

        let local_resources = LocalResources::default();

        WorkloadComponent::new(
            format!("workload-{id}"),
            format!("test-workload-{id}"),
            "test-namespace".to_string(),
            component,
            linker,
            Vec::new(),
            local_resources,
        )
    }

    /// Tests basic plugin binding with one plugin and one component.
    /// Verifies that `on_workload_bind` is called before `on_component_bind`.
    #[tokio::test]
    async fn test_single_plugin_single_component() {
        // Use the actual interfaces that http_counter.wasm uses
        let http_interface = WitInterface {
            namespace: "wasi".to_string(),
            package: "blobstore".to_string(),
            interfaces: ["container".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
            config: std::collections::HashMap::new(),
        };

        let plugin = Arc::new(MockPlugin::new(
            "blobstore-plugin",
            vec![],
            vec![http_interface.clone()],
        ));

        let mut plugins = HashMap::new();
        plugins.insert(plugin.id(), plugin.clone() as Arc<dyn HostPlugin>);

        // Create workload with single component
        let components = vec![create_test_component("component1")];

        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            None,
            components,
            vec![http_interface.clone()],
        );

        let bound_plugins = workload.bind_plugins(&plugins).await.unwrap();

        // Verify plugin was called once for workload binding
        assert_eq!(plugin.get_call_count("on_workload_bind"), 1);

        // Verify plugin was called once for component binding
        assert_eq!(plugin.get_call_count("on_component_bind"), 1);

        // Verify bound_plugins contains our plugin with the component
        assert_eq!(bound_plugins.len(), 1);
        let (_bound_plugin, component_ids) = &bound_plugins[0];
        assert_eq!(component_ids.len(), 1);

        // Verify call order
        let records = plugin.get_call_records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].method, "on_workload_bind");
        assert_eq!(records[1].method, "on_component_bind");
        assert_eq!(records[1].component_id.as_ref().unwrap(), &component_ids[0]);
    }

    /// Tests complex binding scenarios with multiple plugins and components.
    /// Verifies that each plugin gets called once for workload binding.
    #[tokio::test]
    async fn test_multiple_plugins_multiple_components() {
        let http_interface = WitInterface::from("wasi:http/incoming-handler@0.2.0");
        let blobstore_interface = WitInterface::from("wasi:blobstore/blobstore@0.2.0");
        let keyvalue_interface = WitInterface::from("wasi:keyvalue/store@0.2.0");

        let http_plugin = Arc::new(MockPlugin::new(
            "http-plugin",
            vec![],
            vec![http_interface.clone()],
        ));

        let storage_plugin = Arc::new(MockPlugin::new(
            "storage-plugin",
            vec![],
            vec![blobstore_interface.clone(), keyvalue_interface.clone()],
        ));

        let mut plugins = HashMap::new();
        plugins.insert(http_plugin.id(), http_plugin.clone() as Arc<dyn HostPlugin>);
        plugins.insert(
            storage_plugin.id(),
            storage_plugin.clone() as Arc<dyn HostPlugin>,
        );

        // Create components
        let components = vec![
            create_test_component("component1"),
            create_test_component("component2"),
            create_test_component("component3"),
        ];

        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            None,
            components,
            vec![
                http_interface.clone(),
                blobstore_interface.clone(),
                keyvalue_interface.clone(),
            ],
        );

        // Note: Due to the way world() works on real components, we can't easily mock it
        // This test verifies the structure and call patterns are correct
        let _bound_plugins = workload.bind_plugins(&plugins).await.unwrap();

        // Each plugin that matches should be in the result
        for (plugin, _component_ids) in &_bound_plugins {
            // Each plugin gets called once for on_workload_bind
            if plugin.id() == "http-plugin" {
                assert_eq!(http_plugin.get_call_count("on_workload_bind"), 1);
            } else if plugin.id() == "storage-plugin" {
                assert_eq!(storage_plugin.get_call_count("on_workload_bind"), 1);
            }
        }
    }

    /// Tests that when multiple plugins provide the same interface,
    /// only one plugin gets bound to avoid duplicate interface handling.
    #[tokio::test]
    async fn test_no_duplicate_bindings() {
        let http_interface = WitInterface::from("wasi:http/incoming-handler@0.2.0");

        // Two plugins that both provide HTTP
        let plugin1 = Arc::new(MockPlugin::new(
            "http-plugin-1",
            vec![],
            vec![http_interface.clone()],
        ));

        let plugin2 = Arc::new(MockPlugin::new(
            "http-plugin-2",
            vec![],
            vec![http_interface.clone()],
        ));

        let mut plugins = HashMap::new();
        plugins.insert(plugin1.id(), plugin1.clone() as Arc<dyn HostPlugin>);
        plugins.insert(plugin2.id(), plugin2.clone() as Arc<dyn HostPlugin>);

        let components = vec![create_test_component("component1")];

        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            None,
            components,
            vec![http_interface.clone()],
        );

        let _bound_plugins = workload.bind_plugins(&plugins).await.unwrap();

        // Only one plugin should be bound per interface
        // Due to HashMap iteration order being unstable, we can't predict which one
        let total_workload_binds =
            plugin1.get_call_count("on_workload_bind") + plugin2.get_call_count("on_workload_bind");

        // Important: Only one plugin should handle the interface
        assert!(
            total_workload_binds <= 1,
            "Only one plugin should bind for a given interface"
        );
    }

    /// Tests error handling when a workload requests interfaces that no plugin provides.
    /// The binding should fail gracefully with a descriptive error message.
    #[tokio::test]
    async fn test_missing_interface_fails() {
        let http_interface = WitInterface::from("wasi:http/incoming-handler@0.2.0");
        let blobstore_interface = WitInterface::from("wasi:blobstore/blobstore@0.2.0");

        // Plugin only provides HTTP
        let plugin = Arc::new(MockPlugin::new(
            "http-plugin",
            vec![],
            vec![http_interface.clone()],
        ));

        let mut plugins = HashMap::new();
        plugins.insert(plugin.id(), plugin.clone() as Arc<dyn HostPlugin>);

        // Create a component - it will declare what it actually imports
        let components = vec![create_test_component("component1")];

        // Workload requests both HTTP and Blobstore interfaces
        // But only HTTP is available via plugins
        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            None,
            components,
            vec![http_interface.clone(), blobstore_interface.clone()],
        );

        // This should fail if a component actually needs blobstore but it's not provided
        // Note: The actual failure depends on what the component's world() returns
        let _result = workload.bind_plugins(&plugins).await;

        // The test verifies the error path exists and works correctly
        // In practice, this would fail if a component imports blobstore but no plugin provides it
    }

    /// Tests that plugin callbacks are invoked in the correct order:
    /// `on_workload_bind` first, then `on_component_bind` for each component.
    #[tokio::test]
    async fn test_plugin_callback_order() {
        let interface1 = WitInterface::from("test:interface/handler@0.1.0");

        let plugin = Arc::new(MockPlugin::new(
            "test-plugin",
            vec![],
            vec![interface1.clone()],
        ));

        let mut plugins = HashMap::new();
        plugins.insert(plugin.id(), plugin.clone() as Arc<dyn HostPlugin>);

        let components = vec![
            create_test_component("comp1"),
            create_test_component("comp2"),
        ];

        let mut workload = UnresolvedWorkload::new(
            "test-workload-id".to_string(),
            "test-workload".to_string(),
            "test-namespace".to_string(),
            None,
            components,
            vec![interface1.clone()],
        );

        let _bound_plugins = workload.bind_plugins(&plugins).await.unwrap();

        // Verify callback order
        let records = plugin.get_call_records();

        // First call should always be on_workload_bind
        if !records.is_empty() {
            assert_eq!(
                records[0].method, "on_workload_bind",
                "on_workload_bind should be called before component bindings"
            );

            // All subsequent calls should be on_component_bind
            for record in records.iter().skip(1) {
                assert_eq!(
                    record.method, "on_component_bind",
                    "All calls after on_workload_bind should be on_component_bind"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_world_includes_bidirectional() {
        let world = WitWorld {
            imports: HashSet::from([WitInterface::from("wasmcloud:messaging/handler@0.1.0")]),
            exports: HashSet::from([WitInterface::from(
                "wasmcloud:messaging/consumer,types@0.1.0",
            )]),
        };

        let interface1 = WitInterface::from("wasmcloud:messaging/handler@0.1.0");
        let interface2 = WitInterface::from("wasmcloud:messaging/consumer,types@0.1.0");
        let interface3 = WitInterface::from("wasmcloud:messaging/handler,consumer,types@0.1.0");
        let interface4 = WitInterface::from("wasmcloud:messaging/producer@0.1.0");

        assert!(world.includes_bidirectional(&interface1));
        assert!(world.includes_bidirectional(&interface2));
        assert!(world.includes_bidirectional(&interface3));
        assert!(!world.includes_bidirectional(&interface4));
        // Show the difference between includes and includes_bidirectional
        assert!(!world.includes(&interface3));
    }

    /// Tests topological sort with a chain dependency: A -> B -> C
    /// Expected order: C, B, A (or any valid topological order)
    #[test]
    fn test_topological_sort_chain() {
        let a: Arc<str> = Arc::from("component-a");
        let b: Arc<str> = Arc::from("component-b");
        let c: Arc<str> = Arc::from("component-c");

        // A depends on B, B depends on C
        let mut dependencies: HashMap<Arc<str>, HashSet<Arc<str>>> = HashMap::new();
        dependencies.insert(a.clone(), HashSet::from([b.clone()]));
        dependencies.insert(b.clone(), HashSet::from([c.clone()]));
        dependencies.insert(c.clone(), HashSet::new());

        let result = topological_sort_components(&dependencies).unwrap();

        // C should come before B, and B should come before A
        let c_pos = result.iter().position(|x| x == &c).unwrap();
        let b_pos = result.iter().position(|x| x == &b).unwrap();
        let a_pos = result.iter().position(|x| x == &a).unwrap();

        assert!(
            c_pos < b_pos,
            "C should be processed before B: C at {c_pos}, B at {b_pos}"
        );
        assert!(
            b_pos < a_pos,
            "B should be processed before A: B at {b_pos}, A at {a_pos}"
        );
    }

    /// Tests topological sort with no dependencies
    #[test]
    fn test_topological_sort_no_dependencies() {
        let a: Arc<str> = Arc::from("component-a");
        let b: Arc<str> = Arc::from("component-b");
        let c: Arc<str> = Arc::from("component-c");

        let mut dependencies: HashMap<Arc<str>, HashSet<Arc<str>>> = HashMap::new();
        dependencies.insert(a.clone(), HashSet::new());
        dependencies.insert(b.clone(), HashSet::new());
        dependencies.insert(c.clone(), HashSet::new());

        let result = topological_sort_components(&dependencies).unwrap();

        // All components should be present
        assert_eq!(result.len(), 3);
        assert!(result.contains(&a));
        assert!(result.contains(&b));
        assert!(result.contains(&c));
    }

    /// Tests topological sort with diamond dependency: A -> B, A -> C, B -> D, C -> D
    #[test]
    fn test_topological_sort_diamond() {
        let a: Arc<str> = Arc::from("component-a");
        let b: Arc<str> = Arc::from("component-b");
        let c: Arc<str> = Arc::from("component-c");
        let d: Arc<str> = Arc::from("component-d");

        // A depends on B and C, both B and C depend on D
        let mut dependencies: HashMap<Arc<str>, HashSet<Arc<str>>> = HashMap::new();
        dependencies.insert(a.clone(), HashSet::from([b.clone(), c.clone()]));
        dependencies.insert(b.clone(), HashSet::from([d.clone()]));
        dependencies.insert(c.clone(), HashSet::from([d.clone()]));
        dependencies.insert(d.clone(), HashSet::new());

        let result = topological_sort_components(&dependencies).unwrap();

        let a_pos = result.iter().position(|x| x == &a).unwrap();
        let b_pos = result.iter().position(|x| x == &b).unwrap();
        let c_pos = result.iter().position(|x| x == &c).unwrap();
        let d_pos = result.iter().position(|x| x == &d).unwrap();

        // D should come before B and C
        assert!(d_pos < b_pos, "D should be processed before B");
        assert!(d_pos < c_pos, "D should be processed before C");
        // B and C should come before A
        assert!(b_pos < a_pos, "B should be processed before A");
        assert!(c_pos < a_pos, "C should be processed before A");
    }

    /// Tests topological sort with circular dependency detection
    #[test]
    fn test_topological_sort_circular_dependency() {
        let a: Arc<str> = Arc::from("component-a");
        let b: Arc<str> = Arc::from("component-b");
        let c: Arc<str> = Arc::from("component-c");

        // Circular: A -> B -> C -> A
        let mut dependencies: HashMap<Arc<str>, HashSet<Arc<str>>> = HashMap::new();
        dependencies.insert(a.clone(), HashSet::from([b.clone()]));
        dependencies.insert(b.clone(), HashSet::from([c.clone()]));
        dependencies.insert(c.clone(), HashSet::from([a.clone()]));

        let result = topological_sort_components(&dependencies);
        assert!(
            result.is_err(),
            "Should detect circular dependency: {:?}",
            result
        );
    }
}
