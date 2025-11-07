//! WebAssembly component engine for executing workloads.
//!
//! This module provides the core engine functionality for compiling and executing
//! WebAssembly components. The [`Engine`] is responsible for:
//!
//! - Compiling WebAssembly components using wasmtime
//! - Initializing workloads with their components and dependencies
//! - Managing volume mounts and resource configurations
//! - Setting up WASI and HTTP interfaces for components
//!
//! # Key Types
//!
//! - [`Engine`] - The main engine for WebAssembly execution
//! - [`EngineBuilder`] - Builder for configuring engine settings
//! - [`WorkloadComponent`] - Individual components within a workload
//!
//! # Example
//!
//! ```no_run
//! use wash_runtime::engine::Engine;
//! use wash_runtime::types::Workload;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let engine = Engine::builder().build()?;
//! let workload = Workload {
//!     namespace: "default".to_string(),
//!     name: "my-workload".to_string(),
//!     // ... other fields
//! #   annotations: std::collections::HashMap::new(),
//! #   service: None,
//! #   components: vec![],
//! #   host_interfaces: vec![],
//! #   volumes: vec![],
//! };
//!
//! let unresolved = engine.initialize_workload("workload-1", workload)?;
//! // ... bind to plugins and resolve
//! # Ok(())
//! # }
//! ```

use anyhow::{Context, bail};
use wasmtime::PoolingAllocationConfig;
use wasmtime::component::{Component, Linker};

use crate::engine::ctx::Ctx;
use crate::engine::workload::{UnresolvedWorkload, WorkloadComponent, WorkloadService};
use crate::types::{EmptyDirVolume, HostPathVolume, VolumeType, Workload};
use std::path::PathBuf;

pub mod ctx;
mod value;
pub mod workload;

/// The core WebAssembly engine for executing components and workloads.
///
/// The `Engine` is responsible for compiling WebAssembly components, managing
/// their lifecycle, and providing the runtime environment for execution.
/// It wraps a wasmtime engine with additional functionality for workload management.
#[derive(Debug, Clone)]
pub struct Engine {
    // wasmtime engine
    pub(crate) inner: wasmtime::Engine,
}

impl Engine {
    /// Creates a new [`EngineBuilder`] for configuring an engine.
    ///
    /// # Returns
    /// A default `EngineBuilder` that can be customized with additional configuration.
    pub fn builder() -> EngineBuilder {
        EngineBuilder::default()
    }

    /// Gets a reference to the inner wasmtime engine.
    ///
    /// This provides access to the underlying wasmtime engine for advanced use cases.
    ///
    /// # Returns
    /// A reference to the internal `wasmtime::Engine`.
    pub fn inner(&self) -> &wasmtime::Engine {
        &self.inner
    }

    /// Initializes a workload by validating and preparing all its components.
    ///
    /// This function takes a workload definition and prepares it for execution by:
    /// - Validating service components (if present)
    /// - Setting up volumes (both host path and empty directory types)
    /// - Initializing all components with their resource configurations
    ///
    /// # Arguments
    /// * `id` - Unique identifier for this workload instance
    /// * `workload` - The workload configuration containing components, services, and volumes
    ///
    /// # Returns
    /// An `UnresolvedWorkload` that still needs to be bound to plugins and resolved
    /// before execution.
    ///
    /// # Errors
    /// Returns an error if:
    /// - Service component validation fails
    /// - Volume paths don't exist or aren't accessible
    /// - Component initialization fails
    pub fn initialize_workload(
        &self,
        id: impl AsRef<str>,
        workload: Workload,
    ) -> anyhow::Result<UnresolvedWorkload> {
        let Workload {
            namespace,
            name,
            components,
            service,
            volumes,
            host_interfaces,
            ..
        } = workload;

        // Process and validate volumes - create a lookup map from volume name to validated host path
        let mut validated_volumes = std::collections::HashMap::new();

        for v in volumes {
            let host_path = match v.volume_type {
                VolumeType::HostPath(HostPathVolume { local_path }) => {
                    let path = PathBuf::from(&local_path);
                    if !path.is_dir() {
                        anyhow::bail!(
                            "HostPath volume '{local_path}' does not exist or is not a directory",
                        );
                    }
                    path
                }
                VolumeType::EmptyDir(EmptyDirVolume {}) => {
                    // Create a temporary directory for the empty dir volume
                    let temp_dir = tempfile::tempdir()
                        .context("failed to create temp dir for empty dir volume")?;
                    // Persist the temp dir and use the returned host path. Keep returns the
                    // host path to the directory so we can log it for debugging purposes.
                    let kept_path = temp_dir.keep();
                    tracing::info!(host_path = %kept_path.display(), "created and persisted temp dir for empty dir volume");
                    kept_path
                }
            };

            // Store the validated volume for later lookup
            validated_volumes.insert(v.name.clone(), host_path);
        }

        // Iniitalize service
        let service = if let Some(svc) = service {
            match self.initialize_service(id.as_ref(), &name, &namespace, svc, &validated_volumes) {
                Ok(handle) => {
                    tracing::debug!("successfully initialized service component");
                    Some(handle)
                }
                Err(e) => {
                    tracing::error!(err = ?e, "failed to initialize service component");
                    bail!(e);
                }
            }
        } else {
            None
        };

        // Initialize all components
        let mut workload_components = Vec::new();
        for component in components.into_iter() {
            match self.initialize_workload_component(
                id.as_ref(),
                &name,
                &namespace,
                component,
                &validated_volumes,
            ) {
                Ok(handle) => {
                    tracing::debug!("successfully initialized workload component");
                    workload_components.push(handle);
                }
                Err(e) => {
                    tracing::error!(err = ?e, "failed to initialize component");
                    bail!(e);
                }
            }
        }

        Ok(UnresolvedWorkload::new(
            id.as_ref(),
            name,
            namespace,
            service,
            workload_components,
            host_interfaces,
        ))
    }

    fn initialize_service(
        &self,
        workload_id: impl AsRef<str>,
        workload_name: impl AsRef<str>,
        workload_namespace: impl AsRef<str>,
        service: crate::types::Service,
        validated_volumes: &std::collections::HashMap<String, PathBuf>,
    ) -> anyhow::Result<WorkloadService> {
        // Create a wasmtime component from the bytes
        let wasmtime_component = Component::new(&self.inner, service.bytes)
            .context("failed to create component from bytes")?;

        // Create a linker for this component
        let mut linker: Linker<Ctx> = Linker::new(&self.inner);

        // Add WASI@0.2 interfaces to the linker
        wasmtime_wasi::add_to_linker_async(&mut linker).context("failed to add WASI to linker")?;

        // Add HTTP interfaces to the linker if feature is enabled and component uses them
        #[cfg(feature = "wasi-http")]
        if uses_wasi_http(&wasmtime_component) {
            wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)
                .context("failed to add wasi:http/types to linker")?;
        }

        // Build volume mounts for this component by looking up validated volumes
        let mut component_volume_mounts = Vec::new();
        for vm in &service.local_resources.volume_mounts {
            if let Some(host_path) = validated_volumes.get(&vm.name) {
                component_volume_mounts.push((host_path.clone(), vm.clone()));
            } else {
                tracing::warn!(
                    volume = %vm.name,
                    "component references volume that was not found in workload volumes",
                );
            }
        }

        // Create the WorkloadService with volume mounts
        Ok(WorkloadService::new(
            workload_id.as_ref(),
            workload_name.as_ref(),
            workload_namespace.as_ref(),
            wasmtime_component,
            linker,
            component_volume_mounts,
            service.local_resources,
            service.max_restarts,
        ))
    }

    /// Initialize a component that is a part of a workload, add wasi@0.2 interfaces (and
    /// wasi:http if the `http` feature is enabled) to the linker.
    fn initialize_workload_component(
        &self,
        workload_id: impl AsRef<str>,
        workload_name: impl AsRef<str>,
        workload_namespace: impl AsRef<str>,
        component: crate::types::Component,
        validated_volumes: &std::collections::HashMap<String, PathBuf>,
    ) -> anyhow::Result<WorkloadComponent> {
        // Create a wasmtime component from the bytes
        let wasmtime_component = Component::new(&self.inner, component.bytes)
            .context("failed to create component from bytes")?;

        // Create a linker for this component
        let mut linker: Linker<Ctx> = Linker::new(&self.inner);

        // Add WASI@0.2 interfaces to the linker
        wasmtime_wasi::add_to_linker_async(&mut linker).context("failed to add WASI to linker")?;

        // Add HTTP interfaces to the linker
        #[cfg(feature = "wasi-http")]
        if uses_wasi_http(&wasmtime_component) {
            wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)
                .context("failed to add wasi:http/types to linker")?;
        }

        // Build volume mounts for this component by looking up validated volumes
        let mut component_volume_mounts = Vec::new();
        for vm in &component.local_resources.volume_mounts {
            if let Some(host_path) = validated_volumes.get(&vm.name) {
                component_volume_mounts.push((host_path.clone(), vm.clone()));
            } else {
                tracing::warn!(
                    volume = %vm.name,
                    "component references volume that was not found in workload volumes",
                );
            }
        }

        // Create the WorkloadComponent with volume mounts
        Ok(WorkloadComponent::new(
            workload_id.as_ref(),
            workload_name.as_ref(),
            workload_namespace.as_ref(),
            wasmtime_component,
            linker,
            component_volume_mounts,
            component.local_resources,
            // TODO: implement pooling and instance limits
            // component.pool_size,
            // component.max_invocations,
        ))
    }
}

/// Builder for constructing an [`Engine`] with custom configuration.
///
/// The builder pattern allows for flexible configuration of the engine
/// before creation. By default, it enables async support which is required
/// for component execution.
#[derive(Default)]
pub struct EngineBuilder {
    config: wasmtime::Config,
    use_pooling_allocator: Option<bool>,
}

impl EngineBuilder {
    /// Creates a new `EngineBuilder` with default configuration.
    ///
    /// # Returns
    /// A new builder instance with default wasmtime configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Enables or disables the pooling allocator for instance allocation.
    pub fn with_pooling_allocator(mut self, enable: bool) -> Self {
        self.use_pooling_allocator = Some(enable);
        self
    }

    /// Sets a custom wasmtime configuration for the engine.
    ///
    /// This allows full control over the wasmtime engine configuration,
    /// including compilation settings, runtime limits, and feature flags.
    ///
    /// # Arguments
    /// * `config` - A wasmtime `Config` object with custom settings
    ///
    /// # Returns
    /// The builder instance for method chaining.
    pub fn with_config(mut self, config: wasmtime::Config) -> Self {
        self.config = config;
        self
    }
}

impl EngineBuilder {
    /// Builds and returns a configured [`Engine`].
    ///
    /// This method finalizes the configuration and creates the engine.
    /// It automatically enables async support which is required for
    /// component execution.
    ///
    /// # Returns
    /// A new `Engine` instance configured with the builder's settings.
    ///
    /// # Errors
    /// Returns an error if the wasmtime engine creation fails.
    pub fn build(mut self) -> anyhow::Result<Engine> {
        // Async support must be enabled
        self.config.async_support(true);
        // The pooling allocator can be more efficient for workloads with many short-lived instances
        if let Ok(true) = use_pooling_allocator_by_default(self.use_pooling_allocator) {
            tracing::debug!("using pooling allocator by default");
            self.config
                .allocation_strategy(wasmtime::InstanceAllocationStrategy::Pooling(
                    PoolingAllocationConfig::default(),
                ));
        }

        let inner = wasmtime::Engine::new(&self.config)?;
        Ok(Engine { inner })
    }
}

/// Helper function to determine if a component uses wasi:http interfaces
fn uses_wasi_http(component: &Component) -> bool {
    let ty: wasmtime::component::types::Component = component.component_type();
    let engine = component.engine();

    ty.exports(engine)
        .any(|(export, _item)| export.starts_with("wasi:http"))
        || ty
            .imports(engine)
            .any(|(import, _item)| import.starts_with("wasi:http"))
}

// TL;DR this is likely best for machines that can handle the large virtual memory requirement of the pooling allocator
// https://github.com/bytecodealliance/wasmtime/blob/b943666650696f1eb7ff8b217762b58d5ef5779d/src/commands/serve.rs#L641-L656
fn use_pooling_allocator_by_default(enable: Option<bool>) -> anyhow::Result<bool> {
    const BITS_TO_TEST: u32 = 42;
    if let Some(v) = enable {
        return Ok(v);
    }
    let mut config = wasmtime::Config::new();
    config.wasm_memory64(true);
    config.memory_reservation(1 << BITS_TO_TEST);
    let engine = wasmtime::Engine::new(&config)?;
    let mut store = wasmtime::Store::new(&engine, ());
    // NB: the maximum size is in wasm pages to take out the 16-bits of wasm
    // page size here from the maximum size.
    let ty = wasmtime::MemoryType::new64(0, Some(1 << (BITS_TO_TEST - 16)));
    Ok(wasmtime::Memory::new(&mut store, ty).is_ok())
}
