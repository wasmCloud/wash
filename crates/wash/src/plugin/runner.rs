//! Types and implementations for the wasmcloud:wash plugin host interface
//! including the Runner impl on the [`Ctx`].

use anyhow::{Context as _, Result};
use dialoguer::{Confirm, theme::ColorfulTheme};
use std::{collections::HashMap, env, sync::Arc};
use tokio::{process::Command, sync::RwLock};
use tracing::{debug, warn};
use wasmcloud::engine::ctx::Ctx;
use wasmtime::component::Resource;
use wasmtime_wasi::IoView;

use crate::plugin::{PLUGIN_MANAGER_ID, PluginManager, bindings::wasmcloud::wash::types::Metadata};

#[derive(Clone, Debug)]
pub struct Runner {
    #[allow(dead_code)]
    version: String,
    /// The metadata of the plugin
    pub metadata: Metadata,
    pub context: Arc<RwLock<HashMap<String, String>>>,
    pub(crate) skip_confirmation: bool,
}

impl Runner {
    pub fn new(
        metadata: Metadata,
        context: Arc<RwLock<HashMap<String, String>>>,
        skip_confirmation: bool,
    ) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            metadata,
            context,
            skip_confirmation,
        }
    }
}

pub struct ProjectConfig {
    version: String,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            version: "0.1.0".to_string(),
        }
    }
}

pub type Context = Arc<RwLock<HashMap<String, String>>>;
pub type PluginConfig = HashMap<String, String>;

impl crate::plugin::bindings::wasmcloud::wash::types::Host for Ctx {}
/// The Context resource is a passthrough to the same map we use for runtime configuration
impl crate::plugin::bindings::wasmcloud::wash::types::HostContext for Ctx {
    async fn get(&mut self, ctx: Resource<Context>, key: String) -> Option<String> {
        let context = match self.table().get(&ctx) {
            Ok(context) => context,
            Err(e) => {
                tracing::error!(error = %e, "failed to get context resource");
                return None;
            }
        };
        context.read().await.get(&key).cloned()
    }

    async fn set(&mut self, ctx: Resource<Context>, key: String, value: String) -> Option<String> {
        let context = match self.table().get(&ctx) {
            Ok(context) => context,
            Err(e) => {
                tracing::error!(error = %e, "failed to get context resource");
                return None;
            }
        };

        context.write().await.insert(key, value)
    }

    async fn delete(&mut self, ctx: Resource<Context>, key: String) -> Option<String> {
        let context = match self.table().get(&ctx) {
            Ok(context) => context,
            Err(e) => {
                tracing::error!(error = %e, "failed to get context resource");
                return None;
            }
        };

        context.write().await.remove(&key)
    }

    async fn list(&mut self, ctx: Resource<Context>) -> Vec<String> {
        let context = match self.table().get(&ctx) {
            Ok(context) => context,
            Err(e) => {
                tracing::error!(error = %e, "failed to get context resource");
                return vec![];
            }
        };

        context.read().await.keys().cloned().collect()
    }

    async fn drop(&mut self, ctx: Resource<Context>) -> wasmtime::Result<()> {
        self.table()
            .delete(ctx)
            .context("[host-context-drop] deleting context")?;
        Ok(())
    }
}

impl crate::plugin::bindings::wasmcloud::wash::types::HostProjectConfig for Ctx {
    async fn version(&mut self, ctx: Resource<ProjectConfig>) -> String {
        let c = self.table().get(&ctx).unwrap();
        c.version.clone()
    }

    async fn drop(&mut self, ctx: Resource<ProjectConfig>) -> wasmtime::Result<()> {
        self.table()
            .delete(ctx)
            .context("[host-project-config-drop] deleting project config")?;
        Ok(())
    }
}

impl crate::plugin::bindings::wasmcloud::wash::types::HostRunner for Ctx {
    async fn context(&mut self, runner: Resource<Runner>) -> Result<Resource<Context>, String> {
        let runner = self.table.get(&runner).map_err(|e| e.to_string())?;
        self.table
            .push(runner.context.clone())
            .map_err(|e| e.to_string())
    }

    /// The plugin config is a passthrough to the same map we use for runtime configuration
    async fn plugin_config(
        &mut self,
        _ctx: Resource<Runner>,
    ) -> Result<Resource<PluginConfig>, String> {
        let Some(plugin) = self.get_plugin::<PluginManager>(PLUGIN_MANAGER_ID) else {
            return Err("failed to get plugin manager".to_string());
        };

        let runtime_config = {
            let all_config = plugin.runtime_config.read().await;
            match all_config.get(&self.id) {
                Some(config) => config.clone(),
                None => {
                    tracing::warn!(component_id = %self.id, "no plugin config found, returning default");
                    PluginConfig::default()
                }
            }
        };

        self.table.push(runtime_config).map_err(|e| e.to_string())
    }

    async fn host_exec(
        &mut self,
        ctx: Resource<Runner>,
        bin: String,
        args: Vec<String>,
    ) -> Result<(String, String), String> {
        let ctx_data = self.table.get(&ctx).map_err(|e| e.to_string())?;

        // Prompt for confirmation unless explicitly skipped
        if !ctx_data.skip_confirmation {
            // TODO(ISSUE#3): cache this somewhere
            let confirmed = Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt(format!(
                    "{} wants to run `{bin}` with arguments: {args:?}.\nContinue?",
                    ctx_data.metadata.name
                ))
                .default(true)
                .interact()
                .map_err(|e| e.to_string())?;

            if !confirmed {
                debug!(bin = %bin, ?args, "host command execution denied by user");
                return Ok((String::new(), String::new()));
            }
        }

        debug!(bin = %bin, ?args, "executing host command");
        match Command::new(bin).args(args).output().await {
            Ok(output) => {
                let stdout = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
                let stderr = String::from_utf8(output.stderr).map_err(|e| e.to_string())?;
                Ok((stdout, stderr))
            }
            Err(e) => Err(e.to_string()),
        }
    }

    async fn host_exec_background(
        &mut self,
        ctx: Resource<Runner>,
        bin: String,
        args: Vec<String>,
    ) -> Result<(), String> {
        let ctx_data = self.table.get(&ctx).map_err(|e| e.to_string())?;

        // Prompt for confirmation unless explicitly skipped
        if !ctx_data.skip_confirmation {
            // TODO(ISSUE#3): cache this somewhere
            let confirmed = Confirm::with_theme(&ColorfulTheme::default())
                .with_prompt(format!(
                    "{} wants to run `{bin}` with arguments in the background: {args:?}.\nContinue?",
                    ctx_data.metadata.name
                ))
                .default(true)
                .interact()
                .map_err(|e| e.to_string())?;

            if !confirmed {
                debug!(bin = %bin, ?args, "background host command execution denied by user");
                return Ok(());
            }
        }

        debug!(bin = %bin, ?args, "executing host command in background");
        match Command::new(bin).args(args).kill_on_drop(true).spawn() {
            Ok(mut child) => {
                tokio::spawn(async move {
                    if let Err(e) = child.wait().await {
                        warn!(error = %e, "background process exited with error");
                    }
                });
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    async fn drop(&mut self, ctx: Resource<Runner>) -> wasmtime::Result<()> {
        self.table()
            .delete(ctx)
            .context("[host-runner-drop] deleting runner")?;
        Ok(())
    }
}

impl crate::plugin::bindings::wasmcloud::wash::types::HostPluginConfig for Ctx {
    async fn get(&mut self, ctx: Resource<PluginConfig>, key: String) -> Option<String> {
        let plugin_config = match self.table().get(&ctx) {
            Ok(plugin_config) => plugin_config,
            Err(e) => {
                tracing::error!(error = %e, "failed to get plugin config resource");
                return None;
            }
        };
        plugin_config.get(&key).cloned()
    }

    async fn set(
        &mut self,
        ctx: Resource<PluginConfig>,
        key: String,
        value: String,
    ) -> Option<String> {
        let plugin_config = match self.table().get_mut(&ctx) {
            Ok(plugin_config) => plugin_config,
            Err(e) => {
                tracing::error!(error = %e, "failed to get plugin config resource");
                return None;
            }
        };
        plugin_config.insert(key, value)
    }

    async fn delete(&mut self, ctx: Resource<PluginConfig>, key: String) -> Option<String> {
        let plugin_config = match self.table().get_mut(&ctx) {
            Ok(plugin_config) => plugin_config,
            Err(e) => {
                tracing::error!(error = %e, "failed to get plugin config resource");
                return None;
            }
        };
        plugin_config.remove(&key)
    }

    async fn list(&mut self, ctx: Resource<PluginConfig>) -> Vec<String> {
        let plugin_config = match self.table().get(&ctx) {
            Ok(plugin_config) => plugin_config,
            Err(e) => {
                tracing::error!(error = %e, "failed to get plugin config resource");
                return vec![];
            }
        };
        plugin_config.keys().cloned().collect()
    }

    async fn drop(&mut self, ctx: Resource<PluginConfig>) -> wasmtime::Result<()> {
        self.table()
            .delete(ctx)
            .context("[host-plugin-drop] deleting plugin config")?;
        Ok(())
    }
}
