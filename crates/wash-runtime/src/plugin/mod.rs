//! Plugin system for extending host capabilities.
//!
//! This module provides the plugin framework that allows the wasmcloud host
//! to support different WASI interfaces and capabilities. Plugins implement
//! specific functionality that components can use through standard interfaces.
//!
//! # Plugin Architecture
//!
//! Plugins are Rust types that implement the [`HostPlugin`] trait. They:
//! - Declare which WIT interfaces they provide via [`HostPlugin::world`]
//! - Bind to components that need their capabilities via [`HostPlugin::bind_component`]
//! - Can participate in workload lifecycle events
//! - Are automatically linked into the wasmtime runtime
//!
//! # Built-in Plugins
//!
//! The crate provides several built-in plugins for common WASI interfaces:
//! - [`wasi_http`] - HTTP server capabilities (`wasi:http/incoming-handler`)
//! - [`wasi_config`] - Runtime configuration (`wasi:config/runtime`)
//! - [`wasi_blobstore`] - Object storage (`wasi:blobstore`)
//! - [`wasi_keyvalue`] - Key-value storage (`wasi:keyvalue`)
//! - [`wasi_logging`] - Structured logging (`wasi:logging`)

use crate::{
    engine::workload::{ResolvedWorkload, UnresolvedWorkload, WorkloadComponent},
    wit::WitWorld,
};

#[cfg(feature = "wasi-http")]
pub mod wasi_http;

#[cfg(feature = "wasi-config")]
pub mod wasi_config;

#[cfg(feature = "wasi-blobstore")]
pub mod wasi_blobstore;

#[cfg(feature = "wasi-keyvalue")]
pub mod wasi_keyvalue;

#[cfg(feature = "wasi-logging")]
pub mod wasi_logging;

/// The [`HostPlugin`] trait provides an interface for implementing built-in plugins for the host.
/// A plugin is primarily responsible for implementing a specific [`WitWorld`] as a collection of
/// imports and exports that will be directly linked to the workload's [`wasmtime::component::Linker`].
///
/// For example, the runtime doesn't implement `wasi:keyvalue`, but it's a key capability for many component
/// applications. This crate provides a [`wasi_keyvalue::WasiKeyvalue`] built-in that persists key-value data
/// in-memory and implements the component imports of `wasi:keyvalue` atomics, batch and store.
///
/// You can supply your own [`HostPlugin`] implementations to the [`crate::host::HostBuilder::with_plugin`] function.
#[async_trait::async_trait]
pub trait HostPlugin: std::any::Any + Send + Sync + 'static {
    /// Returns the unique identifier for this plugin.
    ///
    /// This ID must be unique across all plugins registered with a host.
    /// It's used to retrieve plugin instances and avoid conflicts.
    ///
    /// # Returns
    /// A static string slice containing the plugin's unique identifier.
    fn id(&self) -> &'static str;

    /// Returns the WIT interfaces that this plugin provides.
    ///
    /// The returned `WitWorld` contains the imports and exports that this plugin
    /// implements. The plugin's `bind_component` method will only be called if
    /// a workload requires one of these interfaces.
    ///
    /// # Returns
    /// A `WitWorld` containing the plugin's imports and exports.
    fn world(&self) -> WitWorld;

    /// Called when the plugin is started during host initialization.
    ///
    /// This method allows plugins to perform any necessary setup before
    /// accepting workloads. The default implementation does nothing.
    ///
    /// # Returns
    /// Ok if the plugin started successfully.
    ///
    /// # Errors
    /// Returns an error if the plugin fails to initialize, which will
    /// prevent the host from starting.
    async fn start(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Called when a workload is binding to this plugin.
    ///
    /// This method is invoked when a workload is in the process of being bound to the plugin,
    /// allowing the plugin to perform any necessary setup or validation before the binding is finalized.
    /// The default implementation does nothing.
    ///
    /// # Arguments
    /// * `workload` - The unresolved workload that is being bound.
    /// * `interfaces` - The set of WIT interfaces that the workload requires from this plugin.
    ///
    /// # Returns
    /// Ok if the binding preparation succeeded.
    ///
    /// # Errors
    /// Returns an error if the plugin cannot support the requested binding.
    async fn on_workload_bind(
        &self,
        _workload: &UnresolvedWorkload,
        _interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Called when a [`WorkloadComponent`] is being bound to this plugin.
    ///
    /// This method is called when a workload requires interfaces that this
    /// plugin provides. The plugin should configure the component's linker
    /// with the necessary implementations.
    ///
    /// # Arguments
    /// * `component` - The workload component to bind to this plugin
    /// * `interfaces` - The specific WIT interfaces the component requires
    ///
    /// # Returns
    /// Ok if binding succeeded.
    ///
    /// # Errors
    /// Returns an error if the plugin cannot bind to the component.
    async fn on_component_bind(
        &self,
        _component: &mut WorkloadComponent,
        _interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Called when a workload has been fully resolved and is ready for use.
    ///
    /// This optional callback allows plugins to perform actions after a workload
    /// has been successfully bound and resolved. The default implementation
    /// does nothing.
    ///
    /// # Arguments
    /// * `workload` - The fully resolved workload
    /// * `component_id` - The ID of the specific component within the workload
    ///
    /// # Returns
    /// Ok if the callback completed successfully.
    ///
    /// # Errors
    /// Returns an error if the plugin fails to handle the resolved workload.
    async fn on_workload_resolved(
        &self,
        _workload: &ResolvedWorkload,
        _component_id: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Called when a workload is being stopped or unbound from this plugin.
    ///
    /// This method allows plugins to clean up any resources associated with
    /// the workload. The default implementation does nothing.
    ///
    /// # Arguments
    /// * `workload` - The workload being unbound
    /// * `interfaces` - The interfaces that were bound
    ///
    /// # Returns
    /// Ok if unbinding succeeded.
    ///
    /// # Errors
    /// Returns an error if cleanup fails.
    async fn on_workload_unbind(
        &self,
        _workload: &ResolvedWorkload,
        _interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Called when the plugin is being stopped during host shutdown.
    ///
    /// This method allows plugins to perform cleanup before the host stops.
    /// The default implementation does nothing.
    ///
    /// # Returns
    /// Ok if the plugin stopped successfully.
    ///
    /// # Errors
    /// Returns an error if cleanup fails (errors are logged but don't prevent shutdown).
    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }
}
