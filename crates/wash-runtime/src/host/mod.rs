//! Host runtime for managing WebAssembly workloads and plugins.
//!
//! The host module provides the runtime environment for executing WebAssembly
//! workloads. It manages the lifecycle of components, coordinates with plugins
//! to provide capabilities, and handles system resources.
//!
//! # Key Components
//!
//! - [`Host`] - The main runtime that manages workloads and plugins
//! - [`HostBuilder`] - Builder for configuring host settings
//! - [`HostApi`] - Trait defining the host's external API
//! - [`HostWorkload`] - Internal representation of workload states
//!
//! # Architecture
//!
//! The host acts as the central coordinator between:
//! - WebAssembly components that need execution
//! - Plugins that provide WASI and other capabilities
//! - System resources like networking and storage
//! - External consumers through the HostApi
//!
//! # Example
//!
//! ```no_run
//! use wash_runtime::host::{HostBuilder, HostApi};
//! use wash_runtime::engine::Engine;
//! use std::sync::Arc;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let engine = Engine::builder().build()?;
//! let host = HostBuilder::new()
//!     .with_engine(engine)
//!     .with_friendly_name("my-host")
//!     .build()?;
//!
//! let host = host.start().await?;
//! let heartbeat = host.heartbeat().await?;
//! println!("Host {} is running", heartbeat.friendly_name);
//! # Ok(())
//! # }
//! ```

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use names::{Generator, Name};
use tokio::sync::RwLock;
use tracing::{debug, trace, warn};

use crate::engine::Engine;
use crate::engine::workload::ResolvedWorkload;
use crate::plugin::HostPlugin;
use crate::types::*;
use crate::wit::WitWorld;

mod sysinfo;
use sysinfo::SystemMonitor;

pub mod http;

/// The API for interacting with a wasmcloud host.
///
/// This trait defines the core operations for managing workloads on a host,
/// including starting, stopping, and querying workload status, as well as
/// retrieving host health information.
pub trait HostApi {
    /// Request a heartbeat containing the host's current state and system information.
    ///
    /// # Returns
    /// A `HostHeartbeat` containing system metrics, version info, and capability information.
    ///
    /// # Errors
    /// Returns an error if system information cannot be retrieved.
    fn heartbeat(&self) -> impl Future<Output = anyhow::Result<HostHeartbeat>>;
    /// Start a new workload on this host.
    ///
    /// # Arguments
    /// * `request` - Contains the workload configuration to start
    ///
    /// # Returns
    /// A `WorkloadStartResponse` with the status of the started workload.
    ///
    /// # Errors
    /// Returns an error if the workload fails to start or validate.
    fn workload_start(
        &self,
        request: WorkloadStartRequest,
    ) -> impl Future<Output = anyhow::Result<WorkloadStartResponse>>;
    /// Query the status of a running workload.
    ///
    /// # Arguments
    /// * `request` - Contains the workload ID to query
    ///
    /// # Returns
    /// A `WorkloadStatusResponse` with the current state of the workload.
    ///
    /// # Errors
    /// Returns an error if the workload is not found.
    fn workload_status(
        &self,
        request: WorkloadStatusRequest,
    ) -> impl Future<Output = anyhow::Result<WorkloadStatusResponse>>;
    /// Stop a running workload on this host.
    ///
    /// # Arguments
    /// * `request` - Contains the workload ID to stop
    ///
    /// # Returns
    /// A `WorkloadStopResponse` with the final status of the stopped workload.
    ///
    /// # Errors
    /// Returns an error if the workload cannot be stopped or is not found.
    fn workload_stop(
        &self,
        request: WorkloadStopRequest,
    ) -> impl Future<Output = anyhow::Result<WorkloadStopResponse>>;
    /// Updates a running workload on this host.
    ///
    /// # Arguments
    /// * `request` - Contains the workload ID to stop
    ///
    /// # Returns
    /// A `WrokloadUpdateResponse` with the final status of the updated workload.
    ///
    /// # Errors
    /// Returns an error if the workload cannot be updated either due to workload not stopping or if it's not found.
    fn workload_update(
        &self,
        request: WorkloadUpdateRequest,
    ) -> impl Future<Output = anyhow::Result<WorkloadUpdateResponse>>;
}

// Helper trait impl that helps with Arc-ing the Host
impl<T: HostApi> HostApi for Arc<T> {
    async fn heartbeat(&self) -> anyhow::Result<HostHeartbeat> {
        self.as_ref().heartbeat().await
    }
    async fn workload_start(
        &self,
        request: WorkloadStartRequest,
    ) -> anyhow::Result<WorkloadStartResponse> {
        self.as_ref().workload_start(request).await
    }
    async fn workload_stop(
        &self,
        request: WorkloadStopRequest,
    ) -> anyhow::Result<WorkloadStopResponse> {
        self.as_ref().workload_stop(request).await
    }
    async fn workload_update(
        &self,
        request: WorkloadUpdateRequest,
    ) -> anyhow::Result<WorkloadUpdateResponse> {
        self.as_ref().workload_update(request).await
    }
    async fn workload_status(
        &self,
        request: WorkloadStatusRequest,
    ) -> anyhow::Result<WorkloadStatusResponse> {
        self.as_ref().workload_status(request).await
    }
}

/// Internal representation of a workload's state within the host.
///
/// This enum tracks the lifecycle stages of a workload from starting
/// through running to stopping or error states.
#[derive(Debug, Clone)]
pub enum HostWorkload {
    Starting,
    // Boxed to reduce size of the enum
    Running(Box<ResolvedWorkload>),
    Stopping,
    Error,
}

impl From<&HostWorkload> for WorkloadState {
    fn from(hw: &HostWorkload) -> Self {
        match hw {
            HostWorkload::Starting => WorkloadState::Starting,
            HostWorkload::Running(_) => WorkloadState::Running,
            HostWorkload::Stopping => WorkloadState::Stopping,
            HostWorkload::Error => WorkloadState::Error,
        }
    }
}

/// A wasmcloud host that manages WebAssembly workloads and plugins.
///
/// The `Host` is the primary runtime for executing workloads. It manages:
/// - An engine for compiling and running WebAssembly components
/// - A collection of workloads and their states
/// - Plugins that extend host functionality
/// - System monitoring and resource tracking
pub struct Host {
    engine: Engine,
    /// Workloads mapped from ID to the workload and its current state
    workloads: Arc<RwLock<HashMap<String, HostWorkload>>>,
    /// Plugins in a map from their ID to the plugin itself
    plugins: HashMap<&'static str, Arc<dyn HostPlugin>>,
    /// Host metadata
    id: String,
    hostname: String,
    friendly_name: String,
    version: String,
    labels: HashMap<String, String>,
    started_at: chrono::DateTime<chrono::Utc>,
    /// System monitor for tracking CPU/memory usage
    system_monitor: Arc<RwLock<SystemMonitor>>,
    // endpoints: HashMap<String, EndpointConfiguration>
    pub(crate) http_handler: std::sync::Arc<dyn crate::host::http::HostHandler>,
    config: HostConfig,
}

impl Host {
    /// Create a new builder for the host.
    pub fn builder() -> HostBuilder {
        HostBuilder::default()
    }

    /// Start the host and initialize all plugins.
    ///
    /// This method must be called before the host can accept workloads.
    /// It starts all registered plugins and prepares the host for operation.
    ///
    /// # Returns
    /// An `Arc` wrapped host ready to accept workloads.
    ///
    /// # Errors
    /// Returns an error if any plugin fails to start.
    pub async fn start(self) -> anyhow::Result<Arc<Self>> {
        self.http_handler
            .start()
            .await
            .context("failed to start HTTP handler")?;

        // Start all plugins, any errors means the host fails to start.
        for (id, plugin) in &self.plugins {
            if let Err(e) = plugin.start().await {
                tracing::error!(id = id, err = ?e, "failed to start plugin");
                bail!(e)
            }
        }

        Ok(Arc::new(self))
    }

    /// Stop the host and shut down all plugins.
    ///
    /// Attempts to gracefully stop all plugins with a 3-second timeout
    /// for each. Errors are logged but don't prevent other plugins from
    /// being stopped.
    ///
    /// # Returns
    /// Ok if the shutdown process completes (even with plugin errors).
    pub async fn stop(self: Arc<Self>) -> anyhow::Result<()> {
        self.http_handler
            .stop()
            .await
            .context("failed to stop HTTP handler")?;

        // Stop all plugins, log errors but continue stopping others
        for (id, plugin) in &self.plugins {
            let stop_fut = plugin.stop();
            match tokio::time::timeout(std::time::Duration::from_secs(3), stop_fut).await {
                Ok(Err(e)) => {
                    tracing::error!(id = id, err = ?e, "failed to stop plugin");
                }
                Err(_) => {
                    tracing::error!(id = id, "plugin stop timed out after 3 seconds");
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Get a label value by key.
    ///
    /// # Arguments
    /// * `label` - The label key to look up
    ///
    /// # Returns
    /// The label value if it exists, None otherwise.
    pub fn label(&self, label: impl AsRef<str>) -> Option<&String> {
        self.labels.get(label.as_ref())
    }

    /// Get the unique identifier for this host.
    ///
    /// # Returns
    /// The host's unique ID string.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get the system hostname for this host.
    ///
    /// # Returns
    /// The host's system hostname string.
    pub fn hostname(&self) -> &str {
        &self.hostname
    }

    /// Get all labels assigned to this host.
    ///
    /// # Returns
    /// A reference to the host's labels map.
    pub fn labels(&self) -> &HashMap<String, String> {
        &self.labels
    }

    /// Get the version of this host.
    ///
    /// # Returns
    /// The host's version string.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Get host config
    ///
    /// # Returns
    /// The host's config
    pub fn config(&self) -> &HostConfig {
        &self.config
    }

    /// Get the human-readable name for this host.
    ///
    /// # Returns
    /// The host's friendly name string.
    pub fn friendly_name(&self) -> &str {
        &self.friendly_name
    }

    /// Returns the WIT (imports, exports) that this host can provide to any component.
    ///
    /// Put another way, this represents a simplified version of the host world. For
    /// example, this WIT world:
    /// ```wit
    /// package wasmcloud:host@0.1.0;
    ///
    /// interface foo {
    /// ...
    /// }
    /// interface bar {
    /// ...
    /// }
    ///
    /// world host {
    ///   import foo;
    ///   export bar;
    /// }
    /// ```
    ///
    /// Would be returned as:
    /// (
    ///  vec![WitInterface { namespace: "wasmcloud", package: "host", interfaces: ["foo"], version: Some("0.1.0") }],
    ///  vec![WitInterface { namespace: "wasmcloud", package: "host", interfaces: ["bar"], version: Some("0.1.0") }],
    /// )
    ///
    /// This can be viewed as an inversion of the worlds that this host can support. In the above example,
    /// this host can support any component that imports `bar` and exports `foo`. Other exports will be ignored,
    /// and other imports that are unsatisfied will be rejected.
    pub fn wit_world(&self) -> WitWorld {
        let mut imports = HashSet::new();
        // The host provides wasi@0.2 interfaces other than wasi:http
        // <https://docs.rs/wasmtime-wasi/36.0.2/wasmtime_wasi/p2/index.html#wasip2-interfaces>
        let mut exports = HashSet::from([
            "wasi:http/types,incoming-handler,outgoing-handler@0.2.0".into(),
            "wasi:io/poll,error,streams@0.2.0".into(),
            "wasi:clocks/monotonic-clock,wall-time@0.2.0".into(),
            "wasi:random/random@0.2.0".into(),
            "wasi:cli/environment,exit,stderr,stdin,stdout,terminal-input,terminal-output,terminal-stderr,terminal-stdin,terminal-stdout@0.2.0".into(),
            "wasi:clocks/monotonic-clock,wall-clock@0.2.0".into(),
            "wasi:filesystem/preopens,types@0.2.0".into(),
            "wasi:random/insecure-seed,insecure,random@0.2.0".into(),
            "wasi:sockets/instance-network,ip-name-lookup,network,tcp-create-socket,tcp,udp-create-socket,udp@0.2.0".into(),
        ]);

        // Include imports and exports that plugins specify
        imports.extend(
            self.plugins
                .values()
                .flat_map(|p| p.world().imports.into_iter().collect::<Vec<_>>()),
        );
        exports.extend(
            self.plugins
                .values()
                .flat_map(|p| p.world().exports.into_iter().collect::<Vec<_>>()),
        );

        WitWorld { imports, exports }
    }

    /// Returns a three-tuple of (OS architecture, OS name, OS kernel)
    async fn get_system_info(&self) -> (String, String, String) {
        // Get OS information
        let os_name = std::env::consts::OS.to_string();
        let os_arch = std::env::consts::ARCH.to_string();
        let os_kernel = std::env::consts::FAMILY.to_string();
        (os_arch, os_name, os_kernel)
    }

    /// Returns a tuple of (total memory, free memory)
    async fn get_memory_info(&self) -> anyhow::Result<(u64, u64)> {
        let monitor = self.system_monitor.read().await;
        let mem = monitor.memory_usage();
        Ok((mem.total_memory, mem.free_memory))
    }

    /// Returns the current global CPU usage as a percentage
    async fn get_cpu_usage(&self) -> anyhow::Result<f32> {
        let monitor = self.system_monitor.read().await;
        Ok(monitor.cpu_usage().global_usage)
    }
}

impl HostApi for Host {
    async fn heartbeat(&self) -> anyhow::Result<HostHeartbeat> {
        // Refresh system info before reporting
        {
            let mut monitor = self.system_monitor.write().await;
            monitor.refresh();
            monitor.report_usage();
        }

        let (os_arch, os_name, os_kernel) = self.get_system_info().await;
        let (system_memory_total, system_memory_free) = self
            .get_memory_info()
            .await
            .context("failed to get memory info")?;
        let system_cpu_usage = self
            .get_cpu_usage()
            .await
            .context("failed to get CPU usage")?;

        // Count components and providers from workloads
        let (workload_count, component_count) = {
            let workloads = self.workloads.read().await;
            let workload_count: u64 = workloads.len() as u64;
            let mut component_count: u64 = 0;
            for workload in workloads.values() {
                if let HostWorkload::Running(workload) = workload {
                    component_count += workload.component_count().await as u64;
                }
            }
            (workload_count, component_count)
        };

        // Collect all imports and exports from the host and plugins
        let mut imports = Vec::new();
        let mut exports = Vec::new();

        for plugin in self.plugins.values() {
            let world = plugin.world();
            imports.extend(world.imports.into_iter());
            exports.extend(world.exports.into_iter());
        }

        Ok(HostHeartbeat {
            id: self.id.clone(),
            hostname: self.hostname.clone(),
            friendly_name: self.friendly_name.clone(),
            version: self.version.clone(),
            labels: self.labels.clone(),
            started_at: self.started_at,
            os_arch,
            os_name,
            os_kernel,
            system_cpu_usage,
            system_memory_total,
            system_memory_free,
            component_count,
            workload_count,
            imports,
            exports,
        })
    }

    /// Start a workload or specific components within a workload
    async fn workload_start(
        &self,
        request: WorkloadStartRequest,
    ) -> anyhow::Result<WorkloadStartResponse> {
        // Check if this is a component-specific start (workload already exists)
        let existing_workload = self
            .workloads
            .read()
            .await
            .get(&request.workload_id)
            .cloned();

        if let Some(component_ids) = &request.component_ids {
            // Component-specific start/restart
            if let Some(HostWorkload::Running(resolved_workload)) = existing_workload {
                debug!(
                    workload_id = request.workload_id,
                    component_ids = ?component_ids,
                    "restarting specific components in workload"
                );

                // Build maps: component_name → component_id and component_id → component_name
                let (name_to_id, id_to_name): (HashMap<String, String>, HashMap<String, String>) = {
                    let components_arc = resolved_workload.components();
                    let components = components_arc.read().await;
                    let mut name_to_id = HashMap::new();
                    let mut id_to_name = HashMap::new();
                    for (id, component) in components.iter() {
                        if let Some(name) = component.metadata().component_name() {
                            name_to_id.insert(name.to_string(), id.to_string());
                            id_to_name.insert(id.to_string(), name.to_string());
                        }
                    }
                    (name_to_id, id_to_name)
                };

                // Resolve component_ids (which can be names OR IDs) to component names
                // We need names to look up in the new spec
                let mut component_names_to_restart = Vec::new();
                let mut name_to_original_id: HashMap<String, String> = HashMap::new();

                for component_id_or_name in component_ids {
                    // Check if it's a name (exists in name_to_id)
                    if let Some(id) = name_to_id.get(component_id_or_name) {
                        component_names_to_restart.push(component_id_or_name.clone());
                        name_to_original_id.insert(component_id_or_name.clone(), id.clone());
                    }
                    // Check if it's an ID (exists in id_to_name)
                    else if let Some(name) = id_to_name.get(component_id_or_name) {
                        component_names_to_restart.push(name.clone());
                        name_to_original_id.insert(name.clone(), component_id_or_name.clone());
                    } else {
                        warn!(
                            workload_id = request.workload_id,
                            component_id_or_name,
                            "component not found by ID or name, skipping restart"
                        );
                    }
                }

                if component_names_to_restart.is_empty() {
                    bail!("no valid components found to restart");
                }

                let components_arc = resolved_workload.components();

                // Mark components as Reconciling and unbind Running components
                {
                    let components = components_arc.read().await;
                    for name in component_names_to_restart.iter() {
                        let component_id = name_to_original_id.get(name).unwrap();
                        if let Some(component) = components.get(component_id.as_str()) {
                            let state = component.get_state().await;

                            // Set to Reconciling
                            component
                                .set_state(crate::types::ComponentState::Reconciling)
                                .await;

                            // Unbind plugins if component is Running
                            if matches!(state, crate::types::ComponentState::Running) {
                                if let Some(plugins) = component.plugins() {
                                    for (plugin_id, plugin) in plugins.iter() {
                                        let world = component.world();
                                        let plugin_world = plugin.world();
                                        let bound_interfaces = world
                                            .imports
                                            .iter()
                                            .filter(|import| plugin_world.imports.contains(import))
                                            .cloned()
                                            .collect::<std::collections::HashSet<_>>();

                                        if let Err(e) = plugin
                                            .on_workload_unbind(
                                                resolved_workload.id(),
                                                bound_interfaces,
                                            )
                                            .await
                                        {
                                            warn!(
                                                plugin_id,
                                                component_id,
                                                workload_id = request.workload_id,
                                                error = ?e,
                                                "failed to unbind plugin from component during restart"
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // Initialize the specific components from the NEW spec by name
                // This correctly matches components regardless of their order in the spec
                let new_components = self.engine.initialize_components_by_name(
                    &request.workload_id,
                    &request.workload,
                    &component_names_to_restart,
                )?;

                // Bind each new component to plugins and add to workload
                let mut restarted_component_ids = Vec::new();
                for (component_name, new_component) in new_components {
                    // Get the original component ID for this name
                    let original_component_id = name_to_original_id
                        .get(&component_name)
                        .cloned()
                        .ok_or_else(|| {
                        anyhow::anyhow!(
                            "internal error: no original component ID for name '{}'",
                            component_name
                        )
                    })?;

                    match resolved_workload
                        .bind_and_add_component(
                            new_component,
                            original_component_id.clone(),
                            &self.plugins,
                            resolved_workload.host_interfaces(),
                        )
                        .await
                    {
                        Ok(_) => {
                            restarted_component_ids.push(original_component_id.clone());
                            debug!(
                                workload_id = request.workload_id,
                                component_id = original_component_id,
                                component_name,
                                "component restarted successfully"
                            );
                        }
                        Err(e) => {
                            warn!(
                                workload_id = request.workload_id,
                                component_id = original_component_id,
                                component_name,
                                error = ?e,
                                "failed to restart component"
                            );
                            // Set component to Error state
                            if let Some(component) = components_arc
                                .read()
                                .await
                                .get(original_component_id.as_str())
                            {
                                component
                                    .set_state(crate::types::ComponentState::Error)
                                    .await;
                            }
                            bail!(e);
                        }
                    }
                }

                // Re-link any components that depend on the updated components
                // This ensures that components importing from updated components
                // get fresh InstancePre references
                if let Err(e) = resolved_workload
                    .relink_dependent_components(&restarted_component_ids)
                    .await
                {
                    warn!(
                        workload_id = request.workload_id,
                        error = ?e,
                        "failed to re-link dependent components after update"
                    );
                    // Continue anyway - the updated components are still in place
                }

                // Refresh the HTTP handler cache so it uses the updated component chain
                if let Err(e) = self
                    .http_handler
                    .refresh_workload_cache(&resolved_workload)
                    .await
                {
                    warn!(
                        workload_id = request.workload_id,
                        error = ?e,
                        "failed to refresh HTTP handler cache after update"
                    );
                }

                // Collect component info after restart
                let components = {
                    let components = components_arc.read().await;
                    let mut component_infos = Vec::new();
                    for (id, component) in components.iter() {
                        component_infos.push(crate::types::ComponentInfo {
                            component_id: id.to_string(),
                            name: component.metadata().component_name().map(|s| s.to_string()),
                            state: component.get_state().await,
                            message: None,
                        });
                    }
                    component_infos
                };

                return Ok(WorkloadStartResponse {
                    workload_status: WorkloadStatus {
                        workload_id: request.workload_id,
                        workload_state: WorkloadState::Running,
                        message: format!(
                            "components {:?} restarted successfully",
                            restarted_component_ids
                        ),
                        components,
                    },
                });
            } else {
                bail!(
                    "cannot restart specific components: workload is not running or does not exist"
                )
            }
        } else {
            // Full workload start (original behavior)
            // Store the workload with initial state
            self.workloads
                .write()
                .await
                .insert(request.workload_id.clone(), HostWorkload::Starting);

            let service_present = request.workload.service.is_some();

            // Initialize the workload using the engine, receiving the unresolved workload
            let unresolved_workload = self
                .engine
                .initialize_workload(&request.workload_id, request.workload)?;

            let mut resolved_workload = unresolved_workload
                .resolve(Some(&self.plugins), self.http_handler.clone())
                .await?;

            // If the service didn't run and we had one, warn
            if resolved_workload.execute_service().await? != service_present {
                warn!(
                    workload_id = request.workload_id,
                    "service did not properly execute"
                );
            }

            // Collect component info before moving resolved_workload
            let components = {
                let components_arc = resolved_workload.components();
                let components = components_arc.read().await;
                let mut component_infos = Vec::new();
                for (id, component) in components.iter() {
                    // Component state is already Running from resolve()
                    component_infos.push(crate::types::ComponentInfo {
                        component_id: id.to_string(),
                        name: component.metadata().component_name().map(|s| s.to_string()),
                        state: component.get_state().await,
                        message: None,
                    });
                }
                component_infos
            };

            // Update the workload state to `Running`
            self.workloads
                .write()
                .await
                .entry(request.workload_id.clone())
                .and_modify(|workload| {
                    *workload = HostWorkload::Running(Box::new(resolved_workload));
                });

            Ok(WorkloadStartResponse {
                workload_status: WorkloadStatus {
                    workload_id: request.workload_id,
                    workload_state: WorkloadState::Running,
                    message: "Workload started successfully".to_string(),
                    components,
                },
            })
        }
    }

    async fn workload_update(
        &self,
        request: WorkloadUpdateRequest,
    ) -> anyhow::Result<WorkloadUpdateResponse> {
        // Deduce which components to update from the workload spec names
        // Each component in the spec must have a name that matches a running component

        // First, get the running workload to build name → id mapping
        let existing_workload = self
            .workloads
            .read()
            .await
            .get(&request.workload_id)
            .cloned();

        let component_names_to_update: Vec<String> = match &existing_workload {
            Some(HostWorkload::Running(resolved_workload)) => {
                // Build name → id map from running components
                let components_arc = resolved_workload.components();
                let components = components_arc.read().await;
                let mut name_to_id: HashMap<String, String> = HashMap::new();
                for (id, component) in components.iter() {
                    if let Some(name) = component.metadata().component_name() {
                        name_to_id.insert(name.to_string(), id.to_string());
                    }
                }
                drop(components);

                // Now match components from the update spec by name
                let mut names_to_update = Vec::new();
                for spec_component in &request.workload.components {
                    match &spec_component.name {
                        Some(name) => {
                            if name_to_id.contains_key(name) {
                                names_to_update.push(name.clone());
                            } else {
                                bail!(
                                    "component '{}' in update spec not found in running workload",
                                    name
                                );
                            }
                        }
                        None => {
                            bail!(
                                "all components in update spec must have a name to identify which component to update"
                            );
                        }
                    }
                }
                names_to_update
            }
            Some(_) => {
                bail!("cannot update workload: workload is not in Running state");
            }
            None => {
                bail!(
                    "cannot update workload: workload '{}' not found",
                    request.workload_id
                );
            }
        };

        if component_names_to_update.is_empty() {
            bail!("no components to update: workload spec is empty");
        }

        // Use workload_start with the deduced component names
        let start_request = WorkloadStartRequest {
            workload_id: request.workload_id,
            workload: request.workload,
            component_ids: Some(component_names_to_update),
        };

        let start_response = self.workload_start(start_request).await?;

        Ok(WorkloadUpdateResponse {
            workload_status: start_response.workload_status,
        })
    }

    async fn workload_status(
        &self,
        request: WorkloadStatusRequest,
    ) -> anyhow::Result<WorkloadStatusResponse> {
        if let Some(workload) = self.workloads.read().await.get(&request.workload_id) {
            let workload_state = workload.into();
            let components = if let HostWorkload::Running(resolved_workload) = workload {
                let components_arc = resolved_workload.components();
                let components = components_arc.read().await;
                let mut component_infos = Vec::new();
                for (id, component) in components.iter() {
                    component_infos.push(crate::types::ComponentInfo {
                        component_id: id.to_string(),
                        name: component.metadata().component_name().map(|s| s.to_string()),
                        state: component.get_state().await,
                        message: None,
                    });
                }
                component_infos
            } else {
                Vec::new()
            };

            Ok(WorkloadStatusResponse {
                workload_status: WorkloadStatus {
                    workload_id: request.workload_id,
                    message: format!("Workload is {workload_state:?}"),
                    workload_state,
                    components,
                },
            })
        } else {
            anyhow::bail!("Workload not found: {}", request.workload_id)
        }
    }

    async fn workload_stop(
        &self,
        request: WorkloadStopRequest,
    ) -> anyhow::Result<WorkloadStopResponse> {
        let has_workload = self
            .workloads
            .read()
            .await
            .contains_key(&request.workload_id);

        if !has_workload {
            return Ok(WorkloadStopResponse {
                workload_status: WorkloadStatus {
                    workload_id: request.workload_id,
                    workload_state: WorkloadState::Unspecified,
                    message: "Workload not found".to_string(),
                    components: Vec::new(),
                },
            });
        }

        // Check if this is a component-specific stop
        if let Some(component_ids) = &request.component_ids {
            // Component-specific stop
            let workload_clone = self
                .workloads
                .read()
                .await
                .get(&request.workload_id)
                .cloned();

            if let Some(HostWorkload::Running(resolved_workload)) = workload_clone {
                debug!(
                    workload_id = request.workload_id,
                    component_ids = ?component_ids,
                    "stopping specific components in workload"
                );

                let components_arc = resolved_workload.components();
                let components = components_arc.write().await;

                for component_id in component_ids {
                    if let Some(component) = components.get(component_id.as_str()) {
                        // Unbind plugins for this specific component
                        if let Some(plugins) = component.plugins() {
                            for (plugin_id, plugin) in plugins.iter() {
                                let world = component.world();
                                let plugin_world = plugin.world();
                                let bound_interfaces = world
                                    .imports
                                    .iter()
                                    .filter(|import| plugin_world.imports.contains(import))
                                    .cloned()
                                    .collect::<std::collections::HashSet<_>>();

                                if let Err(e) = plugin
                                    .on_workload_unbind(resolved_workload.id(), bound_interfaces)
                                    .await
                                {
                                    warn!(
                                        plugin_id,
                                        component_id,
                                        workload_id = request.workload_id,
                                        error = ?e,
                                        "failed to unbind plugin from component during stop"
                                    );
                                }
                            }
                        }

                        // Set state to Stopped but keep component in memory
                        component
                            .set_state(crate::types::ComponentState::Stopped)
                            .await;

                        // Keep the component in memory (don't remove) to allow restart
                        debug!(
                            workload_id = request.workload_id,
                            component_id, "component stopped and kept in memory"
                        );
                    }
                }

                // Collect all component info with their current states
                let mut all_components: Vec<crate::types::ComponentInfo> = Vec::new();
                for (id, component) in components.iter() {
                    all_components.push(crate::types::ComponentInfo {
                        component_id: id.to_string(),
                        name: component.metadata().component_name().map(|s| s.to_string()),
                        state: component.get_state().await,
                        message: None,
                    });
                }

                return Ok(WorkloadStopResponse {
                    workload_status: WorkloadStatus {
                        workload_id: request.workload_id,
                        workload_state: WorkloadState::Running,
                        message: format!("Components {:?} stopped (kept in memory)", component_ids),
                        components: all_components,
                    },
                });
            } else {
                bail!("Cannot stop specific components: workload is not running")
            }
        } else {
            // Full workload stop (original behavior)
            let resolved_workload = {
                let mut workloads = self.workloads.write().await;
                trace!(
                    workload_id = request.workload_id,
                    "updating workload state to stopping"
                );
                // Insert Stopping state, extract the running workload if it was running
                workloads
                    .insert(request.workload_id.clone(), HostWorkload::Stopping)
                    .and_then(|hw| match hw {
                        HostWorkload::Running(rw) => Some(*rw),
                        _ => None,
                    })
            };

            // Stop the workload:
            // 1. Unbind from all plugins
            // 2. Clean up resources (drop will handle wasmtime cleanup)
            // 3. Remove from active workloads
            if let Some(resolved_workload) = resolved_workload {
                debug!(
                    workload_id = request.workload_id,
                    workload_name = resolved_workload.name(),
                    "stopping workload"
                );

                // Stop the service if running
                resolved_workload.stop_service();

                // Unbind all plugins from the workload
                if let Err(e) = resolved_workload.unbind_all_plugins().await {
                    warn!(
                        workload_id = request.workload_id,
                        error = ?e,
                        "error unbinding plugins during workload stop, continuing"
                    );
                }
            }

            // Remove the workload from the active workloads map
            // This will drop the workload and clean up wasmtime resources
            self.workloads.write().await.remove(&request.workload_id);

            debug!(
                workload_id = request.workload_id,
                "workload stopped successfully"
            );
        }

        Ok(WorkloadStopResponse {
            workload_status: WorkloadStatus {
                workload_id: request.workload_id,
                workload_state: WorkloadState::Stopping,
                message: "Workload stopped successfully".to_string(),
                components: Vec::new(),
            },
        })
    }
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host")
            .field("id", &self.id)
            .field("hostname", &self.hostname)
            .field("friendly_name", &self.friendly_name)
            .field("version", &self.version)
            .field("labels", &self.labels)
            .field("started_at", &self.started_at)
            .field("workloads", &self.workloads)
            .finish()
    }
}

/// Config for the [`Host`]
#[derive(Clone, Debug)]
pub struct HostConfig {
    pub allow_oci_insecure: bool,
    pub oci_pull_timeout: Option<Duration>,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            allow_oci_insecure: false,
            oci_pull_timeout: Duration::from_secs(30).into(),
        }
    }
}

/// Builder for the [`Host`]
pub struct HostBuilder {
    id: String,
    engine: Option<Engine>,
    plugins: HashMap<&'static str, Arc<dyn HostPlugin>>,
    hostname: Option<String>,
    friendly_name: Option<String>,
    labels: HashMap<String, String>,
    http_handler: Option<Arc<dyn crate::host::http::HostHandler>>,
    config: Option<HostConfig>,
}

impl Default for HostBuilder {
    fn default() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            engine: Default::default(),
            plugins: Default::default(),
            hostname: Default::default(),
            friendly_name: Default::default(),
            labels: Default::default(),
            http_handler: Default::default(),
            config: Default::default(),
        }
    }
}

impl HostBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn with_engine(mut self, engine: Engine) -> Self {
        self.engine = Some(engine);
        self
    }

    /// Overrides the default HTTP handler.
    pub fn with_http_handler(mut self, handler: Arc<dyn crate::host::http::HostHandler>) -> Self {
        self.http_handler = Some(handler);
        self
    }

    pub fn with_plugin<T: HostPlugin>(mut self, plugin: Arc<T>) -> anyhow::Result<Self> {
        let plugin_id = plugin.id();

        // Check for duplicate plugin IDs
        if self.plugins.contains_key(plugin_id) {
            bail!("Duplicate plugin ID '{plugin_id}' - plugin IDs must be unique");
        }

        self.plugins.insert(plugin_id, plugin);
        Ok(self)
    }

    /// Sets the hostname for this host.
    ///
    /// # Arguments
    /// * `hostname` - The hostname to use
    ///
    /// # Returns
    /// The builder instance for method chaining.
    pub fn with_hostname(mut self, hostname: impl AsRef<str>) -> Self {
        self.hostname = Some(hostname.as_ref().to_string());
        self
    }

    /// Sets a human-readable friendly name for this host.
    ///
    /// # Arguments
    /// * `name` - The friendly name to use
    ///
    /// # Returns
    /// The builder instance for method chaining.
    pub fn with_friendly_name(mut self, name: impl AsRef<str>) -> Self {
        self.friendly_name = Some(name.as_ref().to_string());
        self
    }

    /// Adds a label to the host.
    ///
    /// Labels are key-value pairs that can be used to categorize
    /// or identify the host.
    ///
    /// # Arguments
    /// * `key` - The label key
    /// * `value` - The label value
    ///
    /// # Returns
    /// The builder instance for method chaining.
    pub fn with_label(mut self, key: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        self.labels
            .insert(key.as_ref().to_string(), value.as_ref().to_string());
        self
    }

    pub fn with_config(mut self, config: HostConfig) -> Self {
        self.config.replace(config);
        self
    }

    /// Builds and returns a configured [`Host`].
    ///
    /// This method finalizes the configuration and creates the host.
    /// If no engine is provided, a default engine is created.
    /// If no hostname is provided, the system hostname is used.
    /// If no friendly name is provided, a random name is generated.
    ///
    /// # Returns
    /// A new `Host` instance ready to be started.
    ///
    /// # Errors
    /// Returns an error if the default engine cannot be created (when no engine is provided).
    pub fn build(self) -> anyhow::Result<Host> {
        let engine = if let Some(engine) = self.engine {
            engine
        } else {
            Engine::builder().build()?
        };

        // Get hostname from system if not provided
        let hostname = self.hostname.unwrap_or_else(|| {
            hostname::get()
                .map(|h| h.to_string_lossy().to_string())
                .unwrap_or_else(|_| "unknown".to_string())
        });

        // Generate a friendly name if not provided
        let friendly_name = self.friendly_name.unwrap_or_else(|| {
            let mut generator = Generator::with_naming(Name::Numbered);
            generator
                .next()
                .unwrap_or_else(|| format!("host-{}", uuid::Uuid::new_v4()))
        });

        // Use a null HTTP handler if none provided
        // It will reject any HTTP requests
        let http_handler = match self.http_handler {
            Some(handler) => handler,
            None => Arc::new(crate::host::http::NullServer::default()),
        };

        Ok(Host {
            engine,
            workloads: Arc::default(),
            plugins: self.plugins,
            id: self.id,
            hostname,
            friendly_name,
            version: env!("CARGO_PKG_VERSION").to_string(),
            labels: self.labels,
            started_at: chrono::Utc::now(),
            system_monitor: Arc::new(RwLock::new(SystemMonitor::new())),
            http_handler,
            config: self.config.unwrap_or_default(),
        })
    }
}
