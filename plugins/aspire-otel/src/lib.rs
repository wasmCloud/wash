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
            id: "dev.wasmcloud.aspire-otel".to_string(),
            name: "aspire-otel".to_string(),
            description: "Launches the all-in-one Aspire container with OpenTelemetry support"
                .to_string(),
            contact: "wasmCloud Team".to_string(),
            url: "https://github.com/wasmcloud/wash".to_string(),
            license: "Apache-2.0".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            default_command: None,
            commands: vec![],
            hooks: Some(vec![HookType::BeforeDev, HookType::AfterDev]),
            credentials: None,
        }
    }
    fn hook(r: Runner, ty: HookType) -> anyhow::Result<(), ()> {
        match ty {
            HookType::BeforeDev => {
                log(
                    Level::Info,
                    "aspire-otel",
                    "starting aspire dashboard container",
                );
                r.host_exec(
                    "docker",
                    &[
                        "run".to_string(),
                        "--rm".to_string(),
                        "-it".to_string(),
                        "-p".to_string(),
                        "18888:18888".to_string(),
                        "-p".to_string(),
                        "4318:18890".to_string(),
                        "-d".to_string(),
                        "--name".to_string(),
                        "aspire-dashboard".to_string(),
                        "-e".to_string(),
                        "DOTNET_DASHBOARD_UNSECURED_ALLOW_ANONYMOUS=true".to_string(),
                        "mcr.microsoft.com/dotnet/aspire-dashboard:9.1".to_string(),
                    ],
                )?;
                log(
                    Level::Info,
                    "aspire-otel",
                    "observability dashboard available at http://localhost:18888",
                );
            }
            HookType::AfterDev => {
                log(
                    Level::Info,
                    "aspire-otel",
                    "stopping aspire dashboard container",
                );
                r.host_exec(
                    "docker",
                    &["stop".to_string(), "aspire-dashboard".to_string()],
                )?;
            }
            _ => {
                log(Level::Warn, "aspire-otel", "unsupported hook type");
            }
        }

        Ok(())
    }
    // All of these functions aren't valid for this type of plugin
    fn authorize(_: Runner, _: CredentialType, _: Option<String>) -> anyhow::Result<String, ()> {
        Err(())
    }
    fn initialize(_: Runner) -> anyhow::Result<(), ()> {
        Err(())
    }
    fn run(_: Runner, _: Command) -> anyhow::Result<(), ()> {
        Err(())
    }
}
