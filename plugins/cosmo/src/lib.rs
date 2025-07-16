//! Implementation of `wasmcloud:wash/plugin` for this [`crate::Component`]
mod bindings;

use crate::bindings::{
    wasi::logging::logging::{log, Level},
    wasmcloud::wash::types::{Command, CredentialType, HookType, Metadata, Runner},
};

pub(crate) struct Component;

impl crate::bindings::exports::wasmcloud::wash::plugin::Guest for crate::Component {
    /// Called by wash to retrieve the plugin metadata
    fn info() -> Metadata {
        Metadata {
            id: "dev.cosmonic.cosmo".to_string(),
            name: "cosmo".to_string(),
            description: "Plugin for extending the Wash CLI with Cosmonic-specific functionality"
                .to_string(),
            contact: "Cosmonic Team".to_string(),
            url: "https://github.com/cosmonic/wash".to_string(),
            license: "Apache-2.0".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            default_command: None,
            commands: vec![Command {
                id: "cosmo".to_string(),
                name: "cosmo".to_string(),
                description: "Cosmonic's Wash plugin".to_string(),
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
                log(Level::Info, "cosmo", "generating Cosmonic manifests");
            }
            _ => {
                log(Level::Warn, "cosmo", "unsupported hook type");
            }
        }

        Ok(())
    }
    fn run(r: Runner, cmd: Command) -> anyhow::Result<(), ()> {
        log(Level::Debug, "cosmo", "executing command");
        log(
            Level::Info,
            "cosmo",
            "For more information, visit https://cosmonic.com",
        );
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
