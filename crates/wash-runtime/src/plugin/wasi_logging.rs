//! Structured logging plugin for WebAssembly components.
//!
//! This plugin implements the `wasi:logging/logging@0.1.0-draft` interface,
//! providing components with structured logging capabilities. It bridges
//! component log messages to the host's tracing infrastructure.
//!
//! # Features
//!
//! - Structured logging with multiple log levels
//! - Integration with Rust's `tracing` ecosystem
//! - Contextual logging with component identification
//! - Efficient log message routing
//!
//! # Log Levels
//!
//! The plugin supports the standard WASI logging levels:
//! - Trace: Detailed diagnostic information
//! - Debug: Debug-level messages
//! - Info: General informational messages
//! - Warn: Warning messages
//! - Error: Error messages
//!
//! # Usage
//!
//! Components can use the WASI logging interface to emit structured log
//! messages that will be processed by the host's logging infrastructure.

use std::collections::HashSet;

use anyhow::bail;
use wasmtime::component::HasSelf;

const WASI_LOGGING_ID: &str = "wasi-logging";

use crate::{
    engine::{ctx::Ctx, workload::WorkloadComponent},
    plugin::{HostPlugin, wasi_logging::bindings::wasi::logging::logging::Level},
    wit::{WitInterface, WitWorld},
};

mod bindings {
    wasmtime::component::bindgen!({
        world: "logging",
        imports: { default: async | trappable },
    });
}

/// WASI logging plugin that provides structured logging capabilities.
///
/// This plugin bridges component log messages to the host's tracing infrastructure,
/// allowing WebAssembly components to emit structured log messages that are
/// processed and routed by the host's logging system.
pub struct WasiLogging;

impl bindings::wasi::logging::logging::Host for Ctx {
    async fn log(&mut self, level: Level, context: String, message: String) -> anyhow::Result<()> {
        match level {
            Level::Critical => tracing::error!(id = &self.id, context, "{message}"),
            Level::Error => tracing::error!(id = &self.id, context, "{message}"),
            Level::Warn => tracing::warn!(id = &self.id, context, "{message}"),
            Level::Info => tracing::info!(id = &self.id, context, "{message}"),
            Level::Debug => tracing::debug!(id = &self.id, context, "{message}"),
            Level::Trace => tracing::trace!(id = &self.id, context, "{message}"),
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl HostPlugin for WasiLogging {
    fn id(&self) -> &'static str {
        WASI_LOGGING_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from("wasi:logging/logging")]),
            ..Default::default()
        }
    }

    async fn on_component_bind(
        &self,
        workload_handle: &mut WorkloadComponent,
        interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        // Ensure exactly one interface: "wasi:logging/logging"
        let mut iter = interfaces.iter();
        let Some(interface) = iter.next() else {
            bail!("No interfaces provided; expected wasi:logging/logging");
        };
        if iter.next().is_some()
            || interface.namespace != "wasi"
            || interface.package != "logging"
            || !interface.interfaces.contains("logging")
        {
            bail!(
                "Expected exactly one interface: wasi:logging/logging, got: {:?}",
                interfaces
            );
        }

        // Add `wasi:logging/logging` to the workload's linker
        bindings::wasi::logging::logging::add_to_linker::<_, HasSelf<Ctx>>(
            workload_handle.linker(),
            |ctx| ctx,
        )?;

        Ok(())
    }
}
