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
//! - [`ResolvedWorkload`] - A fully resolved workload ready for execution
//! - [`WorkloadComponent`] - Individual components within a workload
//!
//! # Example
//!
//! ```no_run
//! use wasmcloud::engine::Engine;
//! use wasmcloud::types::Workload;
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
use tracing::warn;
use wasmtime::component::{Component, Linker};

use crate::engine::ctx::Ctx;
use crate::engine::workload::{UnresolvedWorkload, WorkloadComponent};
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

        // Handle optional service - just validate for now, don't create handle yet
        let service = if let Some(svc) = service {
            warn!(
                "services not supported yet, validating that it's a proper component but not starting it"
            );
            // TODO: Don't clone, return the service as a resolved service handle
            let _component = Component::new(&self.inner, svc.bytes.clone())
                .context("failed to validate service component")?;

            Some(svc)
        } else {
            None
        };

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
                    tracing::debug!(path = ?temp_dir.path(), "created temp dir for empty dir volume");
                    temp_dir.keep()
                }
            };

            // Store the validated volume for later lookup
            validated_volumes.insert(v.name.clone(), host_path);
        }

        // Initialize all components
        let mut workload_components = Vec::new();
        for component in components.into_iter() {
            match self.initialize_workload_component(id.as_ref(), component, &validated_volumes) {
                Ok(handle) => {
                    tracing::debug!("successfully initialized component");
                    workload_components.push(handle);
                }
                Err(e) => {
                    tracing::error!(err = ?e, "failed to initialize component");
                    bail!(e);
                }
            }
        }

        Ok(UnresolvedWorkload::new(
            id.as_ref().to_string(),
            name,
            namespace,
            service,
            workload_components,
            host_interfaces,
        ))
    }

    /// Stops a running workload and cleans up its resources.
    ///
    /// TODO: This function is not yet implemented. It will iterate through
    /// all components and unbind them from plugins.
    pub fn stop_workload(&self) {
        // iterate and unbind workload from all plugins
    }

    /// Initialize a component that is a part of a workload, add wasi@0.2 interfaces (and
    /// wasi:http if the `http` feature is enabled) to the linker.
    fn initialize_workload_component(
        &self,
        workload_id: impl AsRef<str>,
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

        // TODO: only if workload declares incoming-handler or outgoing-handler & the component export/imports the interfaces
        // Add HTTP interfaces to the linker
        #[cfg(feature = "wasi-http")]
        wasmtime_wasi_http::add_only_http_to_linker_async(&mut linker)
            .context("failed to add wasi:http/types to linker")?;

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
            workload_id.as_ref().to_string(),
            wasmtime_component,
            linker,
            component_volume_mounts,
            // TODO: implement pooling and instance limits
            // component.pool_size,
            // component.max_invocations,
        ))
    }

    /// Initializes a service component and returns a service handle.
    ///
    /// Services are long-running components that can handle requests and
    /// be restarted if they fail.
    ///
    /// # Arguments
    /// * `_component` - The compiled WebAssembly component to run as a service
    ///
    /// # Returns
    /// Currently returns unit, will return a service handle when implemented.
    ///
    /// # Errors
    /// Will return errors related to service initialization when implemented.
    ///
    /// TODO: Implement service handle creation
    pub fn initialize_service(&self, _component: Component) -> anyhow::Result<()> {
        // TODO: Implement service handle creation
        todo!("Service handle implementation pending")
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
}

impl EngineBuilder {
    /// Creates a new `EngineBuilder` with default configuration.
    ///
    /// # Returns
    /// A new builder instance with default wasmtime configuration.
    pub fn new() -> Self {
        Self::default()
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

        let inner = wasmtime::Engine::new(&self.config)?;
        Ok(Engine { inner })
    }
}
