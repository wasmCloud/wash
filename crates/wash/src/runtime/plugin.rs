//! Types and implementations for the wasmcloud:wash plugin host interface

use anyhow::{Context as _, Result};
use dialoguer::{Confirm, theme::ColorfulTheme};
use std::{collections::HashMap, env, process::Command, sync::Arc};
use tokio::sync::RwLock;
use tracing::debug;
use wasmtime::component::Resource;
use wasmtime_wasi::IoView;

use crate::runtime::{Ctx, bindings::plugin::exports::wasmcloud::wash::plugin::Metadata};

pub struct Runner {
    #[allow(dead_code)]
    version: String,
    /// The metadata of the plugin
    pub metadata: Metadata,
    pub context: Arc<RwLock<HashMap<String, String>>>,
}

impl Runner {
    pub fn new(metadata: Metadata, context: Arc<RwLock<HashMap<String, String>>>) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            metadata,
            context,
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
pub type PluginConfig = Arc<RwLock<HashMap<String, String>>>;

impl crate::runtime::bindings::plugin::wasmcloud::wash::types::Host for Ctx {}
/// The Context resource is a passthrough to the same map we use for runtime configuration
impl crate::runtime::bindings::plugin::wasmcloud::wash::types::HostContext for Ctx {
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

// impl crate::runtime::bindings::plugin_host::wasmcloud::wash::types::HostWashConfig for Ctx {
//     async fn get(
//         &mut self,
//         _ctx: Resource<WashConfig>,
//         _key: String,
//         _default_value: String,
//     ) -> String {
//         String::new()
//     }

//     async fn list(&mut self, _ctx: Resource<WashConfig>) -> Vec<String> {
//         vec![]
//     }

//     async fn drop(&mut self, ctx: Resource<WashConfig>) -> wasmtime::Result<()> {
//         self.table()
//             .delete(ctx)
//             .context("[host-wash-config-drop] deleting wash config")?;
//         Ok(())
//     }
// }

impl crate::runtime::bindings::plugin::wasmcloud::wash::types::HostProjectConfig for Ctx {
    // async fn wash_config_get(
    //     &mut self,
    //     _ctx: Resource<ProjectConfig>,
    //     _key: String,
    //     _default_value: String,
    // ) -> String {
    //     String::new()
    // }

    // async fn wash_config_path(&mut self, _ctx: Resource<ProjectConfig>) -> String {
    //     String::new()
    // }

    // async fn project_config_get(
    //     &mut self,
    //     _ctx: Resource<ProjectConfig>,
    //     _key: String,
    //     _default_value: String,
    // ) -> String {
    //     String::new()
    // }

    // async fn project_path(
    //     &mut self,
    //     _ctx: Resource<ProjectConfig>,
    // ) -> wasmtime::Result<String, ()> {
    //     Ok(String::new())
    // }

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

impl crate::runtime::bindings::plugin::wasmcloud::wash::types::HostRunner for Ctx {
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
        self.table
            .push(self.runtime_config.clone())
            .map_err(|e| e.to_string())
    }

    async fn host_exec(
        &mut self,
        ctx: Resource<Runner>,
        bin: String,
        args: Vec<String>,
    ) -> Result<(String, String), String> {
        let ctx = self.table.get(&ctx).map_err(|e| e.to_string())?;
        // TODO: cache this somewhere
        Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(format!(
                "{} wants to run `{bin}` with arguments: {args:?}.\nContinue?",
                ctx.metadata.name
            ))
            .default(true)
            .interact()
            .map_err(|e| e.to_string())?;
        debug!(bin = %bin, ?args, "executing host command");
        match Command::new(bin).args(args).output() {
            Ok(output) => {
                let stdout = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
                let stderr = String::from_utf8(output.stderr).map_err(|e| e.to_string())?;
                Ok((stdout, stderr))
            }
            Err(e) => Err(e.to_string()),
        }
    }

    async fn output(&mut self, _ctx: Resource<Runner>, output: String) {
        println!("{output}");
    }

    async fn structured_output(
        &mut self,
        _ctx: Resource<Runner>,
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    ) {
        println!("{}", headers.join("\t"));
        for r in rows {
            println!("{}", r.join("\t"));
        }
    }

    async fn error(&mut self, _ctx: Resource<Runner>, message: String) {
        panic!("{}", message);
    }

    async fn drop(&mut self, ctx: Resource<Runner>) -> wasmtime::Result<()> {
        self.table()
            .delete(ctx)
            .context("[host-runner-drop] deleting runner")?;
        Ok(())
    }
}

impl crate::runtime::bindings::plugin::wasmcloud::wash::types::HostPluginConfig for Ctx {
    async fn get(&mut self, ctx: Resource<PluginConfig>, key: String) -> Option<String> {
        let plugin_config = match self.table().get(&ctx) {
            Ok(plugin_config) => plugin_config,
            Err(e) => {
                tracing::error!(error = %e, "failed to get plugin config resource");
                return None;
            }
        };
        plugin_config.read().await.get(&key).cloned()
    }

    async fn set(
        &mut self,
        ctx: Resource<PluginConfig>,
        key: String,
        value: String,
    ) -> Option<String> {
        let plugin_config = match self.table().get(&ctx) {
            Ok(plugin_config) => plugin_config,
            Err(e) => {
                tracing::error!(error = %e, "failed to get plugin config resource");
                return None;
            }
        };
        plugin_config.write().await.insert(key, value)
    }

    async fn delete(&mut self, ctx: Resource<PluginConfig>, key: String) -> Option<String> {
        let plugin_config = match self.table().get(&ctx) {
            Ok(plugin_config) => plugin_config,
            Err(e) => {
                tracing::error!(error = %e, "failed to get plugin config resource");
                return None;
            }
        };
        plugin_config.write().await.remove(&key)
    }

    async fn list(&mut self, ctx: Resource<PluginConfig>) -> Vec<String> {
        let plugin_config = match self.table().get(&ctx) {
            Ok(plugin_config) => plugin_config,
            Err(e) => {
                tracing::error!(error = %e, "failed to get plugin config resource");
                return vec![];
            }
        };
        plugin_config.read().await.keys().cloned().collect()
    }

    async fn drop(&mut self, ctx: Resource<PluginConfig>) -> wasmtime::Result<()> {
        self.table()
            .delete(ctx)
            .context("[host-plugin-drop] deleting plugin config")?;
        Ok(())
    }
}
