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
    use tracing::debug;
    use wasmcloud::wash::types::{CommandArgument, HookType};

    use crate::runtime::bindings::plugin::wasmcloud::wash::types::Metadata;

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

    impl From<&Metadata> for clap::Command {
        fn from(arg: &Metadata) -> Self {
            // Register the information about the plugin command and its subcommands.
            if let Some(cmd) = arg.command.as_ref() {
                // Register command under `wash <command-name>`
                let mut cli_command = clap::Command::new(&cmd.name)
                    .about(&cmd.description)
                    .allow_hyphen_values(true)
                    .disable_help_flag(false)
                    // If arguments are present, display help. Otherwise we can execute without arguments
                    .arg_required_else_help(!cmd.arguments.is_empty())
                    .subcommand_required(false);
                // Populate the CLI command with args and flags
                for argument in &cmd.arguments {
                    cli_command = cli_command.arg(argument)
                }
                for (flag, argument) in &cmd.flags {
                    cli_command = cli_command.arg(Into::<clap::Arg>::into(argument).long(flag))
                }
                cli_command
            } else if !arg.sub_commands.is_empty() {
                // Register subcommands under `wash <plugin-name> <subcommand-name>`
                let mut cli_command = clap::Command::new(arg.name.clone())
                    .about(&arg.description)
                    .allow_hyphen_values(true)
                    .disable_help_flag(false)
                    .arg_required_else_help(false)
                    // If a top level command isn't specified, a subcommand is required
                    .subcommand_required(arg.command.is_none());
                for sub_command in &arg.sub_commands {
                    // Register each subcommand
                    let mut cli_sub_command = clap::Command::new(&sub_command.name)
                        .about(&sub_command.description)
                        .allow_hyphen_values(true)
                        .disable_help_flag(false);
                    // Populate the CLI command with args and flags
                    for argument in &sub_command.arguments {
                        cli_sub_command = cli_sub_command.arg(argument)
                    }
                    for (flag, argument) in &sub_command.flags {
                        cli_sub_command =
                            cli_sub_command.arg(Into::<clap::Arg>::into(argument).long(flag))
                    }
                    cli_command = cli_command.subcommand(cli_sub_command);
                }
                cli_command
            // Backup case
            } else {
                debug!(
                    name = arg.name,
                    "plugin didn't register a command or subcommands, assuming hook"
                );
                clap::Command::new(arg.name.clone()).about(&arg.description)
            }
        }
    }
}
