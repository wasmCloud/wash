//! Implementation of `wasmcloud:wash/plugin` for this [`crate::Component`]
use crate::bindings::wasmcloud::wash::types::{Command, HookType, Metadata, Runner};

impl crate::bindings::exports::wasmcloud::wash::plugin::Guest for crate::Component {
    /// Called by wash to retrieve the plugin metadata
    fn info() -> Metadata {
        Metadata {
            id: "dev.wasmcloud.keyvalue-filesystem".to_string(),
            name: "keyvalue-filesystem".to_string(),
            description: "Implements the wasi:keyvalue API using the wasi:filesystem API"
                .to_string(),
            contact: "wasmCloud Team".to_string(),
            url: "https://github.com/wasmcloud/wash".to_string(),
            license: "Apache-2.0".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            command: None,
            sub_commands: vec![],
            hooks: vec![HookType::DevRegister],
        }
    }
    fn initialize(_: Runner) -> anyhow::Result<String, String> {
        Ok(String::with_capacity(0))
    }
    // All of these functions aren't valid for this type of plugin
    fn run(_: Runner, _: Command) -> anyhow::Result<String, String> {
        Err("no command registered".to_string())
    }
    fn hook(_: Runner, _: HookType) -> anyhow::Result<String, String> {
        Err("invalid hook usage".to_string())
    }
}
