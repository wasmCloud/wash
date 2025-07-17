//! Implementation of `wasmcloud:wash/plugin` for this [`crate::Component`]
mod bindings;

use crate::bindings::{
    wasi::logging::logging::{Level, log},
    wasmcloud::wash::types::{Command, CredentialType, HookType, Metadata, Runner},
};

pub(crate) struct Component;

impl crate::bindings::exports::wasmcloud::wash::plugin::Guest for crate::Component {
    /// Called by wash to retrieve the plugin metadata
    fn info() -> Metadata {
        Metadata {
            id: "dev.wasmcloud.wadm".to_string(),
            name: "wadm".to_string(),
            description: "Generates a wadm manifest after a dev loop, or when pointing to a Wasm component project"
                .to_string(),
            contact: "wasmCloud Team".to_string(),
            url: "https://github.com/wasmcloud/wash".to_string(),
            license: "Apache-2.0".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            default_command: None,
            commands: vec![Command {
                id: "wadm".to_string(),
                name: "wadm".to_string(),
                description: "Generates a wadm manifest for a component or component project".to_string(),
                flags: vec![],
                arguments: vec![],
                usage: vec!["Make sure to drink your Ovaltine!".to_string()],
            }],
            hooks: Some(vec![HookType::AfterDev]),
            credentials: None,
        }
    }
    fn hook(_r: Runner, ty: HookType) -> anyhow::Result<(), ()> {
        match ty {
            HookType::AfterDev => {
                log(Level::Info, "wadm", "generating wadm manifests...");
            }
            _ => {
                log(Level::Warn, "wadm", "unsupported hook type");
            }
        }

        Ok(())
    }
    fn run(r: Runner, cmd: Command) -> anyhow::Result<(), ()> {
        log(Level::Debug, "wadm", "executing command");
        log(Level::Info, "wadm", "generating wadm manifests...");
        Ok(())
    }
    fn initialize(_: Runner) -> anyhow::Result<(), ()> {
        Ok(())
    }
    // All of these functions aren't valid for this type of plugin
    fn authorize(_: Runner, _: CredentialType, _: Option<String>) -> anyhow::Result<String, ()> {
        Err(())
    }
}
