use anyhow::{Context as _, Result};
use std::process::Command;
use wasmtime::component::Resource;
use wasmtime_wasi::{IoImpl, IoView, ResourceTable};

#[derive(Debug, Default, Clone)]
pub struct WasmcloudWashCtx {
    _priv: (),
}

pub trait WasmcloudWashView: IoView {
    fn ctx(&mut self) -> &mut WasmcloudWashCtx;
}

impl<T: ?Sized + WasmcloudWashView> WasmcloudWashView for &mut T {
    fn ctx(&mut self) -> &mut WasmcloudWashCtx {
        T::ctx(self)
    }
}

impl<T: ?Sized + WasmcloudWashView> WasmcloudWashView for Box<T> {
    fn ctx(&mut self) -> &mut WasmcloudWashCtx {
        T::ctx(self)
    }
}

pub struct Runner {
    #[allow(dead_code)]
    version: String,
    #[allow(dead_code)]
    test: String,
}

impl Default for Runner {
    fn default() -> Self {
        Self {
            version: "0.1.0".to_string(),
            test: "test".to_string(),
        }
    }
}

impl Runner {
    #[allow(dead_code)]
    pub fn new<T>(_view: &mut T) -> Result<Resource<Self>, anyhow::Error>
    where
        T: IoView,
    {
        let ctx = Self::default();
        _view
            .table()
            .push(ctx)
            .context("failed to add execution context")
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

impl ProjectConfig {
    #[allow(dead_code)]
    pub fn new<T>(_view: &mut T) -> Result<Resource<Self>, anyhow::Error>
    where
        T: IoView,
    {
        let ctx = Self::default();
        _view
            .table()
            .push(ctx)
            .context("failed to add project config")
    }
}

#[derive(Default)]
pub struct WashConfig {}

impl WashConfig {
    #[allow(dead_code)]
    pub fn new<T>(_view: &mut T) -> Result<Resource<Self>, anyhow::Error>
    where
        T: IoView,
    {
        let ctx = Self::default();
        _view.table().push(ctx).context("failed to add wash config")
    }
}

#[derive(Default)]
pub struct Context {}

impl Context {
    #[allow(dead_code)]
    pub fn new<T>(_view: &mut T) -> Result<Resource<Self>, anyhow::Error>
    where
        T: IoView,
    {
        let ctx = Self::default();
        _view.table().push(ctx).context("failed to add context")
    }
}

#[repr(transparent)]
pub struct WasmcloudWashImpl<T>(pub IoImpl<T>);

impl<T: IoView> IoView for WasmcloudWashImpl<T> {
    fn table(&mut self) -> &mut ResourceTable {
        T::table(&mut self.0.0)
    }
}

impl<T: WasmcloudWashView> WasmcloudWashView for WasmcloudWashImpl<T> {
    fn ctx(&mut self) -> &mut WasmcloudWashCtx {
        self.0.0.ctx()
    }
}

impl<T> crate::runtime::bindings::plugin_host::wasmcloud::wash::types::Host for WasmcloudWashImpl<T> where
    T: WasmcloudWashView
{
}

impl<T> crate::runtime::bindings::plugin_host::wasmcloud::wash::types::HostContext
    for WasmcloudWashImpl<T>
where
    T: WasmcloudWashView,
{
    fn get(
        &mut self,
        _ctx: wasmtime::component::Resource<Context>,
        _key: String,
        _default_value: String,
    ) -> String {
        String::new()
    }

    fn set(
        &mut self,
        _ctx: wasmtime::component::Resource<Context>,
        _key: String,
        _value: String,
    ) -> wasmtime::Result<(), ()> {
        Ok(())
    }

    fn delete(
        &mut self,
        _ctx: wasmtime::component::Resource<Context>,
        _key: String,
    ) -> wasmtime::Result<(), ()> {
        Ok(())
    }

    fn list(&mut self, _ctx: wasmtime::component::Resource<Context>) -> Vec<String> {
        vec![]
    }

    fn drop(&mut self, ctx: wasmtime::component::Resource<Context>) -> wasmtime::Result<()> {
        self.table()
            .delete(ctx)
            .context("[host-context-drop] deleting context")?;
        Ok(())
    }
}

impl<T> crate::runtime::bindings::plugin_host::wasmcloud::wash::types::HostWashConfig
    for WasmcloudWashImpl<T>
where
    T: WasmcloudWashView,
{
    fn get(
        &mut self,
        _ctx: wasmtime::component::Resource<WashConfig>,
        _key: String,
        _default_value: String,
    ) -> String {
        String::new()
    }

    fn list(&mut self, _ctx: wasmtime::component::Resource<WashConfig>) -> Vec<String> {
        vec![]
    }

    fn drop(&mut self, ctx: wasmtime::component::Resource<WashConfig>) -> wasmtime::Result<()> {
        self.table()
            .delete(ctx)
            .context("[host-wash-config-drop] deleting wash config")?;
        Ok(())
    }
}

impl<T> crate::runtime::bindings::plugin_host::wasmcloud::wash::types::HostProjectConfig
    for WasmcloudWashImpl<T>
where
    T: WasmcloudWashView,
{
    fn wash_config(
        &mut self,
        _ctx: wasmtime::component::Resource<ProjectConfig>,
        _key: String,
        _default_value: String,
    ) -> String {
        String::new()
    }

    fn wash_config_path(&mut self, _ctx: wasmtime::component::Resource<ProjectConfig>) -> String {
        String::new()
    }

    fn project_config(
        &mut self,
        _ctx: wasmtime::component::Resource<ProjectConfig>,
        _key: String,
        _default_value: String,
    ) -> String {
        String::new()
    }

    fn project_path(
        &mut self,
        _ctx: wasmtime::component::Resource<ProjectConfig>,
    ) -> wasmtime::Result<String, ()> {
        Ok(String::new())
    }

    fn version(&mut self, ctx: wasmtime::component::Resource<ProjectConfig>) -> String {
        let c = self.table().get(&ctx).unwrap();
        c.version.clone()
    }

    fn drop(&mut self, ctx: wasmtime::component::Resource<ProjectConfig>) -> wasmtime::Result<()> {
        self.table()
            .delete(ctx)
            .context("[host-project-config-drop] deleting project config")?;
        Ok(())
    }
}

impl<T> crate::runtime::bindings::plugin_host::wasmcloud::wash::types::HostRunner
    for WasmcloudWashImpl<T>
where
    T: WasmcloudWashView,
{
    fn project_config(
        &mut self,
        _ctx: wasmtime::component::Resource<Runner>,
    ) -> Option<wasmtime::component::Resource<ProjectConfig>> {
        None
    }

    fn context(
        &mut self,
        _ctx: wasmtime::component::Resource<Runner>,
    ) -> wasmtime::component::Resource<Context> {
        todo!()
    }

    fn plugin_config(
        &mut self,
        _ctx: wasmtime::component::Resource<Runner>,
    ) -> wasmtime::component::Resource<
        crate::runtime::bindings::plugin_host::wasmcloud::wash::types::PluginConfig,
    > {
        todo!()
    }

    fn host_exec(
        &mut self,
        _ctx: wasmtime::component::Resource<Runner>,
        bin: String,
        args: Vec<String>,
    ) -> Result<(String, String), ()> {
        match Command::new(bin).args(args).output() {
            Ok(output) => {
                let stdout = String::from_utf8(output.stdout).unwrap();
                let stderr = String::from_utf8(output.stderr).unwrap();
                Ok((stdout, stderr))
            }
            Err(_) => Err(()),
        }
    }

    fn authorize(
        &mut self,
        _ctx: wasmtime::component::Resource<Runner>,
        _usage: crate::runtime::bindings::plugin_host::wasmcloud::wash::types::CredentialType,
        _resource: Option<String>,
    ) -> Result<String, ()> {
        Ok(String::new())
    }

    fn output(&mut self, _ctx: wasmtime::component::Resource<Runner>, output: String) {
        println!("{}", output);
    }

    fn structured_output(
        &mut self,
        _ctx: wasmtime::component::Resource<Runner>,
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    ) {
        println!("{}", headers.join("\t"));
        for r in rows {
            println!("{}", r.join("\t"));
        }
    }

    fn error(&mut self, _ctx: wasmtime::component::Resource<Runner>, message: String) {
        panic!("{}", message);
    }

    fn drop(&mut self, ctx: wasmtime::component::Resource<Runner>) -> wasmtime::Result<()> {
        self.table()
            .delete(ctx)
            .context("[host-runner-drop] deleting runner")?;
        Ok(())
    }
}

impl<T> crate::runtime::bindings::plugin_host::wasmcloud::wash::types::HostPluginConfig
    for WasmcloudWashImpl<T>
where
    T: WasmcloudWashView,
{
    fn get(
        &mut self,
        _self_: wasmtime::component::Resource<
            crate::runtime::bindings::plugin_host::wasmcloud::wash::types::PluginConfig,
        >,
        _key: String,
        _default_value: String,
    ) -> String {
        String::new()
    }

    fn set(
        &mut self,
        _self_: wasmtime::component::Resource<
            crate::runtime::bindings::plugin_host::wasmcloud::wash::types::PluginConfig,
        >,
        _key: String,
        _value: String,
    ) -> Result<(), ()> {
        Ok(())
    }

    fn delete(
        &mut self,
        _self_: wasmtime::component::Resource<
            crate::runtime::bindings::plugin_host::wasmcloud::wash::types::PluginConfig,
        >,
        _key: String,
    ) -> Result<(), ()> {
        Ok(())
    }

    fn list(
        &mut self,
        _self_: wasmtime::component::Resource<
            crate::runtime::bindings::plugin_host::wasmcloud::wash::types::PluginConfig,
        >,
    ) -> Vec<String> {
        vec![]
    }

    fn drop(
        &mut self,
        rep: wasmtime::component::Resource<
            crate::runtime::bindings::plugin_host::wasmcloud::wash::types::PluginConfig,
        >,
    ) -> wasmtime::Result<()> {
        self.table()
            .delete(rep)
            .context("[host-plugin-drop] deleting plugin config")?;
        Ok(())
    }
}
