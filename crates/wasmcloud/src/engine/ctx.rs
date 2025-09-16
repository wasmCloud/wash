//! Component execution context for wasmtime stores.
//!
//! This module provides the [`Ctx`] type which serves as the store context
//! for wasmtime when executing WebAssembly components. It integrates WASI
//! interfaces, HTTP capabilities, and plugin access into a unified context.

use std::{any::Any, collections::HashMap, sync::Arc};

use wasmtime::component::ResourceTable;
use wasmtime_wasi::{IoView, WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

use crate::plugin::HostPlugin;

/// The context for a component store and linker, providing access to implementations of:
/// - wasi@0.2 interfaces
/// - wasi:http@0.2 interfaces
pub struct Ctx {
    /// Unique identifier for this component instance. This is a [uuid::Uuid::new_v4] string.
    pub id: String,
    /// The resource table used to manage resources in the Wasmtime store.
    pub table: wasmtime::component::ResourceTable,
    /// The WASI context used to provide WASI functionality to the component.
    pub ctx: WasiCtx,
    /// The HTTP context used to provide HTTP functionality to the component.
    pub http: WasiHttpCtx,
    /// Plugin instances stored by string ID for access during component execution.
    /// These all implement the [`HostPlugin`] trait, but they are cast as `Arc<dyn Any + Send + Sync>`
    /// to support downcasting to the specific plugin type in [`Ctx::get_plugin`]
    plugins: HashMap<&'static str, Arc<dyn Any + Send + Sync>>,
}

impl Ctx {
    /// Get a plugin by its string ID and downcast to the expected type
    pub fn get_plugin<T: HostPlugin + 'static>(&self, plugin_id: &str) -> Option<Arc<T>> {
        self.plugins.get(plugin_id)?.clone().downcast().ok()
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
            id: uuid::Uuid::new_v4().to_string(),
            table: ResourceTable::new(),
            ctx: WasiCtxBuilder::new()
                .args(&["main.wasm"])
                // TODO: bring over the latest goodness from wash
                .inherit_stderr()
                .build(),
            http: WasiHttpCtx::new(),
            plugins: HashMap::new(),
        }
    }
}

impl std::fmt::Debug for Ctx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx")
            .field("id", &self.id)
            .field("table", &self.table)
            .finish()
    }
}

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

// Implement WasiHttpView for wasi:http@0.2
impl WasiHttpView for Ctx {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }
}

/// Helper struct to build a [`Ctx`] with a builder pattern
pub struct CtxBuilder {
    id: String,
    ctx: Option<WasiCtx>,
    plugins: HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>,
}

impl Default for CtxBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl CtxBuilder {
    pub fn new() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            ctx: None,
            plugins: HashMap::new(),
        }
    }

    pub fn with_wasi_ctx(mut self, ctx: WasiCtx) -> Self {
        self.ctx = Some(ctx);
        self
    }

    pub fn with_plugins(
        mut self,
        plugins: HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>,
    ) -> Self {
        self.plugins.extend(plugins);
        self
    }

    pub fn build(self) -> Ctx {
        let plugins = self
            .plugins
            .into_iter()
            .map(|(k, v)| (k, v as Arc<dyn Any + Send + Sync>))
            .collect();

        Ctx {
            id: self.id,
            ctx: self.ctx.unwrap_or_else(|| {
                WasiCtxBuilder::new()
                    .args(&["main.wasm"])
                    .inherit_stderr()
                    .build()
            }),
            http: WasiHttpCtx::new(),
            plugins,
            ..Default::default()
        }
    }
}
