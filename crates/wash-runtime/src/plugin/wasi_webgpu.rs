//! # WASI Webgpu Plugin
//!
//! This module implements a webgpu plugin for the wasmCloud runtime,
//! providing the `wasi:webgpu@0.0.1` interfaces.

use std::{collections::HashSet, sync::Arc};

const WASI_WEBGPU_ID: &str = "wasi-webgpu";

use crate::{
    engine::{ctx::SharedCtx, workload::WorkloadComponent},
    plugin::HostPlugin,
    wit::{WitInterface, WitWorld},
};

/// Webgpu plugin
#[derive(Clone)]
pub struct WasiWebGpu {
    pub gpu: Arc<wasi_webgpu_wasmtime::reexports::wgpu_core::global::Global>,
}

/// Backend options for the WasiWebGpu plugin
pub enum WasiWebGpuBackend {
    /// Backend with all available features
    All,
    /// Noop backend for testing purposes. It does not perform any real GPU operations.
    Noop,
}

impl WasiWebGpu {
    pub fn new(backend: WasiWebGpuBackend) -> Self {
        let (backends, backend_options) = match backend {
            WasiWebGpuBackend::All => (
                wasi_webgpu_wasmtime::reexports::wgpu_types::Backends::all(),
                wasi_webgpu_wasmtime::reexports::wgpu_types::BackendOptions::default(),
            ),
            WasiWebGpuBackend::Noop => (
                wasi_webgpu_wasmtime::reexports::wgpu_types::Backends::NOOP,
                wasi_webgpu_wasmtime::reexports::wgpu_types::BackendOptions {
                    noop: wasi_webgpu_wasmtime::reexports::wgpu_types::NoopBackendOptions {
                        enable: true,
                    },
                    ..Default::default()
                },
            ),
        };

        Self {
            gpu: Arc::new(wasi_webgpu_wasmtime::reexports::wgpu_core::global::Global::new(
                "webgpu",
                &wasi_webgpu_wasmtime::reexports::wgpu_types::InstanceDescriptor {
                    backends,
                    backend_options,
                    flags: wasi_webgpu_wasmtime::reexports::wgpu_types::InstanceFlags::from_build_config(),
                    memory_budget_thresholds: Default::default(),
                },
            )),
        }
    }
}

impl Default for WasiWebGpu {
    fn default() -> Self {
        Self::new(WasiWebGpuBackend::All)
    }
}

impl wasi_graphics_context_wasmtime::WasiGraphicsContextView for SharedCtx {}

struct UiThreadSpawner;
impl wasi_webgpu_wasmtime::MainThreadSpawner for UiThreadSpawner {
    async fn spawn<F, T>(&self, f: F) -> T
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        f()
    }
}

impl wasi_webgpu_wasmtime::WasiWebGpuView for SharedCtx {
    fn instance(&self) -> Arc<wasi_webgpu_wasmtime::reexports::wgpu_core::global::Global> {
        let plugin = self
            .active_ctx
            .get_plugin::<WasiWebGpu>(WASI_WEBGPU_ID)
            .unwrap();
        Arc::clone(&plugin.gpu)
    }

    fn ui_thread_spawner(&self) -> Box<impl wasi_webgpu_wasmtime::MainThreadSpawner + 'static> {
        Box::new(UiThreadSpawner)
    }
}

#[async_trait::async_trait]
impl HostPlugin for WasiWebGpu {
    fn id(&self) -> &'static str {
        WASI_WEBGPU_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            exports: HashSet::from([
                WitInterface::from("wasi:graphics-context/graphics-context"),
                WitInterface::from("wasi:webgpu/webgpu"),
            ]),
            ..Default::default()
        }
    }

    async fn on_component_bind(
        &self,
        component: &mut WorkloadComponent,
        interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        // Check if any of the interfaces are wasi:webgpu related
        let has_webgpu = interfaces
            .iter()
            .any(|i| i.namespace == "wasi" && i.package == "webgpu");

        if !has_webgpu {
            tracing::warn!(
                "WasiWebgpu plugin requested for non-wasi:webgpu interface(s): {:?}",
                interfaces
            );
            return Ok(());
        }

        tracing::debug!(
            workload_id = component.id(),
            "Adding webgpu interfaces to linker for workload"
        );
        let linker = component.linker();

        wasi_webgpu_wasmtime::add_to_linker(linker)?;
        wasi_graphics_context_wasmtime::add_to_linker(linker)?;

        let id = component.id();
        tracing::debug!(
            workload_id = id,
            "Successfully added webgpu interfaces to linker for workload"
        );

        Ok(())
    }
}
