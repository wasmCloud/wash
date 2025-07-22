//! Implementation of `wasmcloud:wash/plugin` for this [`crate::Component`]
use crate::bindings::wasmcloud::wash::types::{
    Command, CredentialType, HookType, Metadata, Runner,
};

impl crate::bindings::exports::wasmcloud::wash::plugin::Guest for crate::Component {
    /// Called by wash to retrieve the plugin metadata
    fn info() -> Metadata {
        Metadata {
            id: "dev.wasmcloud.blobstore-filesystem".to_string(),
            name: "blobstore-filesystem".to_string(),
            description: "Implements the wasi:blobstore API using the wasi:filesystem API"
                .to_string(),
            contact: "wasmCloud Team".to_string(),
            url: "https://github.com/wasmcloud/wash".to_string(),
            license: "Apache-2.0".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            default_command: None,
            commands: vec![],
            hooks: Some(vec![HookType::DevRegister]),
        }
    }
    fn initialize(_: Runner) -> anyhow::Result<(), ()> {
        Ok(())
    }
    // All of these functions aren't valid for this type of plugin
    fn run(_: Runner, _: Command) -> anyhow::Result<(), ()> {
        Err(())
    }
    fn hook(_: Runner, _: HookType) -> anyhow::Result<(), ()> {
        Err(())
    }
}
