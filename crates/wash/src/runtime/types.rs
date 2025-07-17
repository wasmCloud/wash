use anyhow::{Context as _, Result};
use dialoguer::{Confirm, theme::ColorfulTheme};
use std::{env, process::Command};
use tracing::debug;
use wasmtime::component::Resource;
use wasmtime_wasi::IoView;

use crate::runtime::{Ctx, bindings::plugin_host::exports::wasmcloud::wash::plugin::Metadata};

pub struct Runner {
    #[allow(dead_code)]
    version: String,
    /// The metadata of the plugin
    pub metadata: Metadata,
}

impl Runner {
    pub fn new(metadata: Metadata) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            metadata,
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

#[derive(Default)]
pub struct WashConfig {}

#[derive(Default)]
pub struct Context {}

impl crate::runtime::bindings::plugin_host::wasmcloud::wash::types::Host for Ctx {}
impl crate::runtime::bindings::plugin_host::wasmcloud::wash::types::HostContext for Ctx {
    async fn get(
        &mut self,
        _ctx: Resource<Context>,
        _key: String,
        _default_value: String,
    ) -> String {
        String::new()
    }

    async fn set(
        &mut self,
        _ctx: Resource<Context>,
        _key: String,
        _value: String,
    ) -> wasmtime::Result<(), ()> {
        Ok(())
    }

    async fn delete(&mut self, _ctx: Resource<Context>, _key: String) -> wasmtime::Result<(), ()> {
        Ok(())
    }

    async fn list(&mut self, _ctx: Resource<Context>) -> Vec<String> {
        vec![]
    }

    async fn drop(&mut self, ctx: Resource<Context>) -> wasmtime::Result<()> {
        self.table()
            .delete(ctx)
            .context("[host-context-drop] deleting context")?;
        Ok(())
    }
}

impl crate::runtime::bindings::plugin_host::wasmcloud::wash::types::HostWashConfig for Ctx {
    async fn get(
        &mut self,
        _ctx: Resource<WashConfig>,
        _key: String,
        _default_value: String,
    ) -> String {
        String::new()
    }

    async fn list(&mut self, _ctx: Resource<WashConfig>) -> Vec<String> {
        vec![]
    }

    async fn drop(&mut self, ctx: Resource<WashConfig>) -> wasmtime::Result<()> {
        self.table()
            .delete(ctx)
            .context("[host-wash-config-drop] deleting wash config")?;
        Ok(())
    }
}

impl crate::runtime::bindings::plugin_host::wasmcloud::wash::types::HostProjectConfig for Ctx {
    async fn wash_config_get(
        &mut self,
        _ctx: Resource<ProjectConfig>,
        _key: String,
        _default_value: String,
    ) -> String {
        String::new()
    }

    async fn wash_config_path(&mut self, _ctx: Resource<ProjectConfig>) -> String {
        String::new()
    }

    async fn project_config_get(
        &mut self,
        _ctx: Resource<ProjectConfig>,
        _key: String,
        _default_value: String,
    ) -> String {
        String::new()
    }

    async fn project_path(
        &mut self,
        _ctx: Resource<ProjectConfig>,
    ) -> wasmtime::Result<String, ()> {
        Ok(String::new())
    }

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

impl crate::runtime::bindings::plugin_host::wasmcloud::wash::types::HostRunner for Ctx {
    async fn project_config(&mut self, _ctx: Resource<Runner>) -> Option<Resource<ProjectConfig>> {
        None
    }

    async fn context(&mut self, _ctx: Resource<Runner>) -> Resource<Context> {
        todo!()
    }

    async fn plugin_config(
        &mut self,
        _ctx: Resource<Runner>,
    ) -> Resource<crate::runtime::bindings::plugin_host::wasmcloud::wash::types::PluginConfig> {
        todo!()
    }

    async fn host_exec(
        &mut self,
        ctx: Resource<Runner>,
        bin: String,
        args: Vec<String>,
    ) -> Result<(String, String), ()> {
        let ctx = self.table.get(&ctx).map_err(|_| ())?;
        Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt(format!(
                "{} wants to run `{bin}` with arguments: {args:?}.\nContinue?",
                ctx.metadata.name
            ))
            .default(true)
            .interact()
            .map_err(|_| ())?;
        debug!(bin = %bin, ?args, "executing host command");
        match Command::new(bin).args(args).output() {
            Ok(output) => {
                let stdout = String::from_utf8(output.stdout).map_err(|_| ())?;
                let stderr = String::from_utf8(output.stderr).map_err(|_| ())?;
                Ok((stdout, stderr))
            }
            Err(_) => Err(()),
        }
    }

    async fn authorize(
        &mut self,
        _ctx: Resource<Runner>,
        _usage: crate::runtime::bindings::plugin_host::wasmcloud::wash::types::CredentialType,
        _resource: Option<String>,
    ) -> Result<String, ()> {
        Ok(String::new())
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

impl crate::runtime::bindings::plugin_host::wasmcloud::wash::types::HostPluginConfig for Ctx {
    async fn get(
        &mut self,
        _self_: Resource<
            crate::runtime::bindings::plugin_host::wasmcloud::wash::types::PluginConfig,
        >,
        _key: String,
        _default_value: String,
    ) -> String {
        String::new()
    }

    async fn set(
        &mut self,
        _self_: Resource<
            crate::runtime::bindings::plugin_host::wasmcloud::wash::types::PluginConfig,
        >,
        _key: String,
        _value: String,
    ) -> Result<(), ()> {
        Ok(())
    }

    async fn delete(
        &mut self,
        _self_: Resource<
            crate::runtime::bindings::plugin_host::wasmcloud::wash::types::PluginConfig,
        >,
        _key: String,
    ) -> Result<(), ()> {
        Ok(())
    }

    async fn list(
        &mut self,
        _self_: Resource<
            crate::runtime::bindings::plugin_host::wasmcloud::wash::types::PluginConfig,
        >,
    ) -> Vec<String> {
        vec![]
    }

    async fn drop(
        &mut self,
        rep: Resource<crate::runtime::bindings::plugin_host::wasmcloud::wash::types::PluginConfig>,
    ) -> wasmtime::Result<()> {
        self.table()
            .delete(rep)
            .context("[host-plugin-drop] deleting plugin config")?;
        Ok(())
    }
}
