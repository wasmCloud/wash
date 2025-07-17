//! Implementation of `wasmcloud:wash/plugin` for this [`crate::Component`]
mod bindings;

use crate::bindings::{
    wasi::logging::logging::{log, Level},
    wasmcloud::wash::types::{
        Command, CommandArgument, CredentialType, HookType, Metadata, Runner,
    },
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
                flags: vec![
                    (
                        "generate".to_string(),
                        CommandArgument {
                            name: "generate".to_string(),
                            description: "Generate a wadm manifest for the given component or project".to_string(),
                            is_path: false,
                            required: false,
                            default_value: Some("true".to_string()),
                            value: "false".to_string(),
                        }
                    ),
                    (
                        "dry-run".to_string(),
                        CommandArgument {
                            name: "dry-run".to_string(),
                            description: "Print the manifest to stdout instead of writing to disk".to_string(),
                            is_path: false,
                            required: false,
                            default_value: Some("false".to_string()),
                            value: "false".to_string(),
                        }
                    ),
                ],
                arguments: vec![
                    CommandArgument {
                        name: "project-path".to_string(),
                        description: "Path to the component project directory".to_string(),
                        is_path: true,
                        required: false,
                        default_value: None,
                        value: "".to_string(),
                    }
                ],
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
