use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use anyhow::bail;
use clap::{Args, Subcommand};
use etcetera::AppStrategy;
use hyper::server::conn::http1;
use serde_json::json;
use tokio::net::TcpListener;
use tracing::error;
use tracing::{debug, instrument, trace, warn};
use wasmtime::AsContextMut;
use wasmtime_wasi::DirPerms;
use wasmtime_wasi::FilePerms;
use wasmtime_wasi::WasiCtx;
use wasmtime_wasi_http::io::TokioIo;

use crate::{
    cli::{CliCommand, CliContext, CommandOutput, OutputKind, component_build::build_component},
    plugin::{
        InstallPluginOptions, PluginComponent, install_plugin, list_plugins, sanitize_plugin_name,
        uninstall_plugin,
    },
    runtime::{
        Ctx,
        bindings::plugin::{WashPlugin, exports::wasmcloud::wash::plugin::HookType},
        plugin::Runner,
        prepare_component_plugin,
    },
};

#[derive(Subcommand, Debug, Clone)]
pub enum PluginCommand {
    /// Install a plugin from an OCI reference or file
    Install(InstallCommand),
    /// Uninstall a plugin
    Uninstall(UninstallCommand),
    /// List installed plugins
    List(ListCommand),
    /// Test run a plugin component to inspect its metadata or run commands and hooks
    Test(TestCommand),
}

impl CliCommand for PluginCommand {
    /// Handle the plugin command
    #[instrument(level = "debug", skip_all, name = "plugin")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        match self {
            PluginCommand::Install(cmd) => cmd.handle(ctx).await,
            PluginCommand::Uninstall(cmd) => cmd.handle(ctx).await,
            PluginCommand::List(cmd) => cmd.handle(ctx).await,
            PluginCommand::Test(cmd) => cmd.handle(ctx).await,
        }
    }
}

/// A component plugin command is a parsed Vec of strings which represents the command to be executed.
/// Clap's `external_subcommand` macro supports this by allowing us to pass a Vec<String> as the command.
pub struct ComponentPluginCommand<'a> {
    pub args: &'a Vec<String>,
    pub plugin_component: Option<&'a PluginComponent>,
}

impl<'a> From<&'a Vec<String>> for ComponentPluginCommand<'a> {
    fn from(args: &'a Vec<String>) -> Self {
        Self {
            args,
            plugin_component: None,
        }
    }
}

/// This implementation allows for arguments parsed by clap to locate and execute a plugin command.
impl<'a> CliCommand for ComponentPluginCommand<'a> {
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let mut cli_args = self.args.iter();
        let command_name = cli_args.next();
        let subcommand_name = cli_args.next();

        // Find the plugin component where its metadata name matches the command, or use the provided
        // plugin component if specified.
        let plugin_component = match self.plugin_component {
            Some(pc) => pc,
            None => {
                let command_name = command_name.context(
                    "no command provided to plugin, use --help to see available commands",
                )?;
                if let Some(plugin) = ctx
                    .plugin_manager()
                    .get_commands()
                    .into_iter()
                    .find(|p| &p.metadata.name == command_name)
                {
                    plugin
                } else {
                    anyhow::bail!("no plugin command found for `{command_name}`");
                }
            }
        };

        // Registering the plugin command as a command or subcommand
        // just depends on where we place the flags and args in the clap structure.
        let name = &plugin_component.metadata.name;
        let mut cli_command = clap::Command::new(name)
            .about(&plugin_component.metadata.description)
            .allow_hyphen_values(true)
            .disable_help_flag(false)
            // If a top level command isn't specified, a subcommand is required
            .subcommand_required(plugin_component.metadata.command.is_none());

        // Register the information about the plugin command and its subcommands. This returns an optional
        // command as we should be able to print the help text about the plugin command even if no args are provided.
        let run_command = if let Some(cmd) = plugin_component.metadata.command.as_ref() {
            // Populate the CLI command with args and flags
            for argument in &cmd.arguments {
                cli_command = cli_command.arg(argument)
            }
            for (flag, argument) in &cmd.flags {
                cli_command = cli_command.arg(Into::<clap::Arg>::into(argument).long(flag))
            }

            Some(cmd.to_owned())
        } else if !plugin_component.metadata.sub_commands.is_empty() {
            let mut matched_subcommand = None;
            for sub_command in &plugin_component.metadata.sub_commands {
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
                if Some(&sub_command.name) == subcommand_name {
                    matched_subcommand = Some(sub_command.to_owned());
                }
            }

            matched_subcommand
        } else {
            bail!("no command or subcommand found for `{name}`");
        };

        // TODO: if args.len == 1 and also command is some, then run the command
        // Print help if no args are provided or if --help or -h is used
        if self.args.len() <= 1
            || self.args.contains(&"--help".to_string())
            || self.args.contains(&"-h".to_string())
        {
            return cli_command
                .print_help()
                .map(|()| CommandOutput::ok("", None))
                .context("failed to print help for command");
        }

        let mut run_command =
            run_command.context("failed to find command or subcommand for plugin")?;

        // Populate command args with user-provided arguments
        // TODO: this might need to pop off the first one or something depending on subcommand handling
        // TODO: no unwrap
        let cli_args = cli_command.clone().get_matches_from(self.args.iter());
        let cli_args = if cli_args.subcommand().is_some() {
            cli_args
                .subcommand_matches(subcommand_name.unwrap())
                .unwrap()
                .clone()
        } else {
            cli_args
        };
        trace!("{cli_args:?}");
        for arg in run_command.arguments.iter_mut() {
            if let Ok(Some(value)) = cli_args.try_get_one::<String>(arg.name.as_str()) {
                trace!(
                    ?arg.name,
                    ?value,
                    "found argument value in command matches, updating command argument",
                );
                arg.value = Some(value.to_owned());
            }
        }
        for (flag, arg) in run_command.flags.iter_mut() {
            if let Ok(Some(value)) = cli_args.try_get_one::<String>(flag.as_str()) {
                trace!(
                    ?flag,
                    ?value,
                    "found flag value in command matches, updating command flag",
                );
                arg.value = Some(value.to_owned());
            }
        }

        // TODO: move this inside of the plugin component struct
        tokio::fs::create_dir_all(plugin_component.fs_root.as_path())
            .await
            .context("failed to create plugin filesystem root directory")?;
        let component = &plugin_component.component;
        let mut store = component.new_store(
            Ctx::builder()
                // TODO: Security pass, consider environment, stdio/out/err, etc
                .with_wasi_ctx(
                    WasiCtx::builder()
                        .preopened_dir(
                            plugin_component.fs_root.as_path(),
                            "/tmp",
                            DirPerms::all(),
                            FilePerms::all(),
                        )?
                        .build(),
                )
                .build(),
        );
        let pre = component.instance_pre();
        let instance = pre.instantiate_async(&mut store).await?;

        // The HTTP export is used to handle HTTP requests if the component supports it. It's the same
        // instance as the plugin guest
        // let http_guest = wasmtime_wasi_http::bindings::ProxyPre::new(pre.to_owned())
        //     .context("Failed to get HTTP export from component instance")
        //     .expect("foo");

        // TODO: Only do this if the component has an HTTP export
        // TODO: get this or let user specify this?
        let listener = TcpListener::bind("127.0.0.1:8888").await?;
        // Using the same runtime config for both components so they can run independently
        let runtime_config = store.data().runtime_config.clone();
        let component = plugin_component.component.clone();
        let fs_root = plugin_component.fs_root.clone();
        tokio::spawn(async move {
            loop {
                let runtime_config = runtime_config.clone();
                let component = component.clone();
                let fs_root = fs_root.clone();
                let (client, addr) = listener.accept().await.unwrap();
                if let Err(e) = http1::Builder::new()
                    .keep_alive(true)
                    .serve_connection(
                        TokioIo::new(client),
                        hyper::service::service_fn(move |req| {
                            let mut ctx = Ctx::builder()
                                // TODO: Security pass, consider environment, stdio/out/err, etc
                                .with_wasi_ctx(
                                    WasiCtx::builder()
                                        .preopened_dir(
                                            fs_root.as_path(),
                                            "/tmp",
                                            DirPerms::all(),
                                            FilePerms::all(),
                                        )
                                        .expect("failed to create WASI context")
                                        .build(),
                                )
                                .build();
                            // NOTE: Slightly weird construction to pass the exact same config
                            ctx.runtime_config = runtime_config.clone();
                            let mut store = component.new_store(ctx);
                            let pre = component.instance_pre().to_owned();
                            async move {
                                crate::cli::dev::handle_request(store.as_context_mut(), pre, req)
                                    .await
                            }
                        }),
                    )
                    .await
                {
                    error!(addr = ?addr, err = ?e, "error serving client");
                }
            }
        });

        // Setup Plugin guest instance, call run
        let runner = store.data_mut().table.push(Runner::new(
            plugin_component.metadata.clone(),
            // TODO: context, all plugins get a directory to use?
            Arc::default(),
        ))?;

        // TODO: With how often we use this, a struct is warranted with helper impls
        let plugin_guest = WashPlugin::new(&mut store, &instance)?;
        let guest = plugin_guest.wasmcloud_wash_plugin();

        // Instantiate and run plugin
        match guest.call_run(&mut store, runner, &run_command).await {
            Ok(Ok(res)) => {
                debug!(name = ?name, "command executed");
                // Prepare output data for both text and JSON output
                let output_data = json!({
                    "command": name,
                    "args": self.args,
                    "plugin": plugin_component.metadata.name,
                    "description": run_command.description,
                    "output": res,
                    "success": true
                });

                Ok(CommandOutput::ok(res, Some(output_data)))
            }
            Ok(Err(e)) => Ok(CommandOutput::error(e, None)),
            // TODO: fill out more info
            Err(e) => Ok(CommandOutput::error(e, None)),
        }
    }
}

#[derive(Args, Debug, Clone)]
pub struct InstallCommand {
    /// The source to install from (OCI reference or file path)
    #[clap(name = "source")]
    source: String,
    /// Force overwrite if plugin already exists
    #[clap(short, long)]
    force: bool,
}

#[derive(Args, Debug, Clone)]
pub struct UninstallCommand {
    /// The plugin name to uninstall
    #[clap(name = "name")]
    name: String,
}

#[derive(Args, Debug, Clone)]
pub struct ListCommand {
    /// Output format (text or json)
    #[clap(short, long, default_value = "text")]
    output: OutputKind,
}

#[derive(Args, Debug, Clone)]
pub struct TestCommand {
    /// Path to the component or component project to test
    #[clap(name = "plugin")]
    pub plugin: PathBuf,
    /// The hook types to test
    #[clap(name = "type", long = "hook", conflicts_with = "arg")]
    pub hooks: Vec<HookType>,
    /// The arguments to pass to the plugin command
    #[clap(name = "arg", conflicts_with = "type", trailing_var_arg = true)]
    pub args: Vec<String>,
}

impl InstallCommand {
    /// Handle the plugin install command
    #[instrument(level = "debug", skip_all, name = "plugin_install")]
    pub async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let options = InstallPluginOptions {
            source: self.source.clone(),
            force: self.force,
        };

        match install_plugin(ctx, options).await {
            Ok(result) => Ok(CommandOutput::ok(
                format!(
                    "Plugin '{}' installed successfully from '{}'",
                    result.name, result.source
                ),
                Some(json!({
                    "name": result.name,
                    "source": result.source,
                    "path": result.path,
                    "size": result.size,
                    "success": true
                })),
            )),
            Err(e) => Ok(CommandOutput::error(
                format!("Failed to install plugin: {e}"),
                None,
            )),
        }
    }
}

impl UninstallCommand {
    /// Handle the plugin uninstall command
    #[instrument(level = "debug", skip_all, name = "plugin_uninstall")]
    pub async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        match uninstall_plugin(ctx, &self.name).await {
            Ok(()) => Ok(CommandOutput::ok(
                format!("Plugin '{}' uninstalled successfully", self.name),
                Some(json!({
                    "name": self.name,
                    "success": true
                })),
            )),
            Err(e) => Ok(CommandOutput::error(
                format!("Failed to uninstall plugin '{}': {e}", self.name),
                None,
            )),
        }
    }
}

impl ListCommand {
    /// Handle the plugin list command
    #[instrument(level = "debug", skip_all, name = "plugin_list")]
    pub async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let plugins = list_plugins(ctx.runtime(), ctx.data_dir()).await?;

        match self.output {
            OutputKind::Text => {
                if plugins.is_empty() {
                    Ok(CommandOutput::ok("No plugins installed", None))
                } else {
                    let mut output = String::new();
                    output.push_str("Installed plugins:\n");
                    for (_, plugin) in &plugins {
                        let detail = if !plugin.hooks.is_empty() {
                            format!(
                                "Hooks: {}\n",
                                plugin
                                    .hooks
                                    .iter()
                                    .map(|h| h.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        } else if let Some(cmd) = plugin.command.as_ref() {
                            format!("Command: {}\n", cmd.name)
                        } else if !plugin.sub_commands.is_empty() {
                            format!(
                                "Subcommands: {}\n",
                                plugin
                                    .sub_commands
                                    .iter()
                                    .map(|c| c.name.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        } else {
                            String::new()
                        };
                        output.push_str(&format!(
                            "  {}\n    {}    Description: {}\n    Version: {}\n",
                            plugin.name, detail, plugin.description, plugin.version,
                        ));
                    }
                    Ok(CommandOutput::ok(output.trim_end(), None))
                }
            }
            OutputKind::Json => Ok(CommandOutput::ok(
                "",
                Some(json!({
                    "plugins": plugins,
                    "count": plugins.len()
                })),
            )),
        }
    }
}

impl TestCommand {
    /// Handle the plugin test command
    #[instrument(level = "debug", skip_all, name = "plugin_test")]
    pub async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let wasm = if self.plugin.is_dir() {
            let config = ctx
                .ensure_config(Some(self.plugin.as_path()))
                .await
                .context("failed to load config")?;
            let built_path = build_component(&self.plugin, ctx, &config)
                .await
                .context("Failed to build component from directory")?;
            tokio::fs::read(&built_path.artifact_path)
                .await
                .context("Failed to read built component file")?
        } else {
            tokio::fs::read(&self.plugin)
                .await
                .context("Failed to read component file")?
            // TODO: support OCI references too
        };

        let mut output = String::new();

        let component = prepare_component_plugin(ctx.runtime(), &wasm).await?;
        let mut store = component.new_store(Ctx::default());
        let instance = component
            .instance_pre()
            .instantiate_async(&mut store)
            .await
            .context("Failed to instantiate component")?;
        let plugin_guest =
            WashPlugin::new(&mut store, &instance).context("Failed to get plugin_guest export")?;
        let guest = plugin_guest.wasmcloud_wash_plugin();

        let metadata = guest.call_info(&mut store).await?;
        debug!(metadata = ?metadata, "plugin metadata");

        for name in &self.hooks {
            if let Some(hook) = metadata.hooks.iter().find(|h| h == &name) {
                let runner = store
                    .data_mut()
                    .table
                    // TODO: provide hook context
                    .push(Runner::new(metadata.clone(), Arc::default()))?;
                match guest
                    .call_hook(&mut store, runner, hook.to_owned())
                    .await
                    .context("failed to run hook")?
                {
                    Ok(out) => {
                        output.push_str(out.as_str());
                        output.push_str(&format!("Hook '{name}' executed successfully"));
                    }
                    Err(e) => {
                        warn!(err = e, name = %name, "Hook execution failed");
                        output.push_str(&format!("Hook '{name}' execution failed\n"));
                    }
                }
            } else {
                warn!(name = %name, "Hook not found in plugin metadata");
            }
        }

        // Handle the command if no hooks were executed
        if output.is_empty() {
            let plugin_component = PluginComponent {
                component: Arc::new(component),
                fs_root: ctx
                    .data_dir()
                    .join("plugins")
                    .join("fs")
                    .join(sanitize_plugin_name(&metadata.name)),
                metadata: metadata.clone(),
            };
            let component_plugin_command = ComponentPluginCommand {
                args: &self.args,
                plugin_component: Some(&plugin_component),
            };
            output.push_str(component_plugin_command.handle(ctx).await?.message.as_str());
        }

        Ok(CommandOutput::ok(
            output,
            Some(json!({
                "plugin_path": self.plugin.to_string_lossy(),
                "args": self.args,
                "hooks": self.hooks,
                "metadata": metadata,
                "success": true
            })),
        ))
    }
}
