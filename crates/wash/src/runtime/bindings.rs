//! This module contains all the Wasmtime component bindings for the Wash runtime.

/// Generated bindings used to call components that are being developed with `wash dev`
pub mod dev {
    wasmtime::component::bindgen!({
        path: "./wit",
        world: "dev",
        async: true,
        trappable_imports: true,
        with: {
           "wasi:http/types": wasmtime_wasi_http::bindings::http::types,
           "wasi:io": wasmtime_wasi::bindings::io,
        },
    });
}

/// Generated bindings used to call components that implement the `wasmcloud:wash/plugin` interface
pub mod plugin {
    wasmtime::component::bindgen!({
        path: "./wit",
        world: "wash-plugin",
        additional_derives: [serde::Serialize],
        async: true,
        with: {
            "wasmcloud:wash/types/runner": crate::runtime::plugin::Runner,
            "wasmcloud:wash/types/project-config": crate::runtime::plugin::ProjectConfig,
            "wasmcloud:wash/types/plugin-config": crate::runtime::plugin::PluginConfig,
            "wasmcloud:wash/types/context": crate::runtime::plugin::Context,
        }
    });

    // Using module imports here to keep the top-level `use` statements clean
    use std::fmt::Display;
    use wasmcloud::wash::types::{CommandArgument, HookType};

    impl Display for HookType {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let s = match self {
                HookType::Unknown => "Unknown",
                HookType::BeforeDoctor => "BeforeDoctor",
                HookType::AfterDoctor => "AfterDoctor",
                HookType::BeforeBuild => "BeforeBuild",
                HookType::AfterBuild => "AfterBuild",
                HookType::BeforePush => "BeforePush",
                HookType::AfterPush => "AfterPush",
                HookType::DevRegister => "DevRegister",
                HookType::BeforeDev => "BeforeDev",
                HookType::AfterDev => "AfterDev",
            };
            write!(f, "{s}")
        }
    }

    impl From<&str> for HookType {
        fn from(s: &str) -> Self {
            match s.to_ascii_lowercase().as_str() {
                "beforedoctor" => HookType::BeforeDoctor,
                "afterdoctor" => HookType::AfterDoctor,
                "beforebuild" => HookType::BeforeBuild,
                "afterbuild" => HookType::AfterBuild,
                "beforepush" => HookType::BeforePush,
                "afterpush" => HookType::AfterPush,
                "devregister" => HookType::DevRegister,
                "beforedev" => HookType::BeforeDev,
                "afterdev" => HookType::AfterDev,
                "unknown" => HookType::Unknown,
                _ => HookType::Unknown, // Default case for unknown strings
            }
        }
    }

    /// Easy conversion from the generated argument structure to a Clap argument.
    impl From<&CommandArgument> for clap::Arg {
        fn from(arg: &CommandArgument) -> Self {
            let mut cli_arg = clap::Arg::new(&arg.name)
                .help(&arg.description)
                .required(arg.default.is_none());

            if let Some(default_value) = &arg.default {
                cli_arg = cli_arg.default_value(default_value);
            }

            if let Some(env) = &arg.env {
                cli_arg = cli_arg.env(env);
            }

            cli_arg
        }
    }
}
