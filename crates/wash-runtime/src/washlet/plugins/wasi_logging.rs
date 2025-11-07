//! # WASI Logging Plugin
//!
//! This module routes logging calls from WASI components to the host's tracing
//! system. It implements the `wasi:logging/logging` interface, allowing
//! components to log messages at various levels (trace, debug, info, warn,
//! error, critical).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::engine::ctx::Ctx;
use crate::engine::workload::WorkloadComponent;
use crate::plugin::HostPlugin;
use crate::wit::{WitInterface, WitWorld};
use anyhow::bail;

const PLUGIN_LOGGING_ID: &str = "wasi-logging";

mod bindings {
    crate::wasmtime::component::bindgen!({
        world: "logging",
        imports: { default: async | trappable },
    });
}

use bindings::wasi::logging::logging::Level;
use tokio::sync::RwLock;
use wasmtime::component::HasSelf;

type ComponentMap = Arc<RwLock<HashMap<String, ComponentInfo>>>;

#[derive(Default)]
pub struct TracingLogging {
    components: ComponentMap,
}

struct ComponentInfo {
    workload_name: String,
    workload_namespace: String,
    component_id: String,
}

impl bindings::wasi::logging::logging::Host for Ctx {
    async fn log(&mut self, level: Level, context: String, message: String) -> anyhow::Result<()> {
        let Some(plugin) = self.get_plugin::<TracingLogging>(PLUGIN_LOGGING_ID) else {
            bail!("TracingLogging plugin not found in context");
        };

        let workloads = plugin.components.read().await;
        let Some(ComponentInfo {
            workload_name,
            workload_namespace,
            component_id,
        }) = workloads.get(&self.component_id.to_string())
        else {
            bail!("Component not found in TracingLogging plugin");
        };
        match level {
            Level::Trace => {
                tracing::trace!(
                    workload.component_id = component_id,
                    workload.name = workload_name,
                    workload.namespace = workload_namespace,
                    context,
                    "{message}"
                )
            }
            Level::Debug => {
                tracing::debug!(
                    workload.component_id = component_id,
                    workload.name = workload_name,
                    workload.namespace = workload_namespace,
                    context,
                    "{message}"
                )
            }
            Level::Info => {
                tracing::info!(
                    workload.component_id = component_id,
                    workload.name = workload_name,
                    workload.namespace = workload_namespace,
                    context,
                    "{message}"
                )
            }
            Level::Warn => {
                tracing::warn!(
                    workload.component_id = component_id,
                    workload.name = workload_name,
                    workload.namespace = workload_namespace,
                    context,
                    "{message}"
                )
            }
            Level::Error => {
                tracing::error!(
                    workload.component_id = component_id,
                    workload.name = workload_name,
                    workload.namespace = workload_namespace,
                    context,
                    "{message}"
                )
            }
            Level::Critical => {
                tracing::error!(
                    workload.component_id = component_id,
                    workload.name = workload_name,
                    workload.namespace = workload_namespace,
                    context,
                    "{message}"
                )
            }
        };

        Ok(())
    }
}

#[async_trait::async_trait]
impl HostPlugin for TracingLogging {
    fn id(&self) -> &'static str {
        PLUGIN_LOGGING_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from("wasi:logging/logging")]),
            ..Default::default()
        }
    }

    async fn on_component_bind(
        &self,
        component: &mut WorkloadComponent,
        interfaces: std::collections::HashSet<WitInterface>,
    ) -> anyhow::Result<()> {
        // Ensure exactly one interface: "wasi:logging/logging"
        let has_logging = interfaces
            .iter()
            .any(|i| i.namespace == "wasi" && i.package == "logging");

        if !has_logging {
            tracing::warn!(
                "TracingLoggingPlugin plugin requested for non-wasi:logging interface(s): {:?}",
                interfaces
            );
            return Ok(());
        }

        // Add `wasi:logging/logging` to the workload's linker
        bindings::wasi::logging::logging::add_to_linker::<_, HasSelf<Ctx>>(
            component.linker(),
            |ctx| ctx,
        )?;

        tracing::info!(
            "TracingLoggingPlugin bound to component {} of workload {} in namespace {}",
            component.id(),
            component.workload_name(),
            component.workload_namespace()
        );

        self.components.write().await.insert(
            component.id().to_string(),
            ComponentInfo {
                workload_name: component.workload_name().to_string(),
                workload_namespace: component.workload_namespace().to_string(),
                component_id: component.id().to_string(),
            },
        );

        Ok(())
    }
}
