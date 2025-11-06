//! # WASI Runtime Config Plugin
//!
//! Copies the environment variables provided to each workload to
//! `wasi:config/store`
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::RwLock;

const PLUGIN_RUNTIME_CONFIG_ID: &str = "wasi-config";

use crate::engine::ctx::Ctx;
use crate::engine::workload::WorkloadComponent;
use crate::plugin::HostPlugin;
use crate::wit::{WitInterface, WitWorld};

mod bindings {
    wasmtime::component::bindgen!({
        world: "config",
        trappable_imports: true,
        async: true,
    });
}

use bindings::wasi::config::store::{Error as ConfigError, Host};

/// Runtime configuration plugin that provides access to configuration data.
///
/// This plugin implements the WASI config interface, allowing components to
/// retrieve configuration values at runtime (separate from wasi:cli/env). Each
/// component gets isolated access to its own configuration scope.
#[derive(Clone, Default)]
pub struct RuntimeConfig {
    /// A map of configuration from workload id to key-value pairs
    config: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
}

impl Host for Ctx {
    async fn get(&mut self, key: String) -> anyhow::Result<Result<Option<String>, ConfigError>> {
        let Some(plugin) = self.get_plugin::<RuntimeConfig>(PLUGIN_RUNTIME_CONFIG_ID) else {
            return Ok(Ok(None));
        };
        let config_guard = plugin.config.read().await;
        config_guard
            .get(&self.component_id.to_string())
            .and_then(|map| map.get(&key).cloned())
            .map_or(Ok(Ok(None)), |v| Ok(Ok(Some(v))))
    }

    async fn get_all(&mut self) -> anyhow::Result<Result<Vec<(String, String)>, ConfigError>> {
        let Some(plugin) = self.get_plugin::<RuntimeConfig>(PLUGIN_RUNTIME_CONFIG_ID) else {
            return Ok(Ok(vec![]));
        };
        let config_guard = plugin.config.read().await;
        let entries = config_guard
            .get(&self.component_id.to_string())
            .map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        Ok(Ok(entries))
    }
}

#[async_trait::async_trait]
impl HostPlugin for RuntimeConfig {
    fn id(&self) -> &'static str {
        PLUGIN_RUNTIME_CONFIG_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from("wasi:config/store@0.2.0-rc.1")]),
            exports: HashSet::new(),
        }
    }
    async fn on_component_bind(
        &self,
        component_handle: &mut WorkloadComponent,
        interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        // Find the "wasi:config/store" interface, if present
        let Some(_) = interfaces.iter().find(|i| {
            i.namespace == "wasi" && i.package == "config" && i.interfaces.contains("store")
        }) else {
            // Log a warning if the requested interfaces are not wasi:config/store
            tracing::warn!(
                "RuntimeConfig plugin requested for non-wasi:config/store interface(s): {:?}",
                interfaces
            );
            return Ok(());
        };

        // Add `wasi:config/store` to the workload's linker
        bindings::wasi::config::store::add_to_linker(component_handle.linker(), |ctx| ctx)?;

        // Store the configuration for lookups later
        // This mirrors wasi:cli/env on wasi:config/store
        self.config.write().await.insert(
            component_handle.id().to_string(),
            component_handle.local_resources().environment.clone(),
        );

        Ok(())
    }
}
