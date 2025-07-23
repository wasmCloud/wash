use std::collections::HashMap;
use std::{path::PathBuf, sync::Arc};

use anyhow::Context;
use anyhow::bail;
use clap::{Args, Subcommand};
use etcetera::AppStrategy;
use hyper::server::conn::http1;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tracing::error;
use tracing::{debug, instrument, trace, warn};
use wasmtime::AsContextMut;
use wasmtime_wasi::DirPerms;
use wasmtime_wasi::FilePerms;
use wasmtime_wasi::WasiCtx;
use wasmtime_wasi_http::io::TokioIo;

use crate::runtime::bindings::plugin::wasmcloud::wash::types::Metadata;
use crate::{
    cli::{CliCommand, CliContext, CommandOutput, OutputKind, component_build::build_component},
    plugin::{
        InstallPluginOptions, PluginComponent, install_plugin, list_plugins, uninstall_plugin,
    },
    runtime::{
        Ctx, bindings::plugin::exports::wasmcloud::wash::plugin::HookType, prepare_component_plugin,
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
    /// Test run a plugin component to inspect its metadata or run commands and hooks.
    /// This parses the arguments just like wash does for an installed plugin, so it's
    /// functionally equivalent to run `wash foo bar --arg 1` and `wash plugin test ./component.wasm bar --arg 1`
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
#[derive(Debug)]
pub struct ComponentPluginCommand<'a> {
    pub command_name: &'a str,
    pub args: &'a clap::ArgMatches,
    pub plugin_component: Option<Arc<PluginComponent>>,
}

impl<'a> ComponentPluginCommand<'a> {
    pub fn new(command_name: &'a str, args: &'a clap::ArgMatches) -> Self {
        Self {
            command_name,
            args,
            plugin_component: None,
        }
    }
}

/// This implementation allows for arguments parsed by clap to locate and execute a plugin command.
impl<'a> CliCommand for ComponentPluginCommand<'a> {
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        // Find the plugin component where its metadata name matches the command, or use the provided
        // plugin component if specified.
        let plugin_component = match self.plugin_component.as_ref() {
            Some(pc) => pc.clone(),
            None => ctx
                .plugin_manager()
                .get_command(self.command_name)
                .with_context(|| format!("no plugin command found for `{}`", self.command_name))?,
        };

        // Registering the plugin command as a command or subcommand
        // just depends on where we place the flags and args in the clap structure.
        let name = &plugin_component.metadata.name;

        // Register the information about the plugin command and its subcommands. This returns an optional
        // command as we should be able to print the help text about the plugin command even if no args are provided.
        let (mut run_command, run_args) =
            if let Some(cmd) = plugin_component.metadata.command.as_ref() {
                (cmd.to_owned(), self.args)
            } else if !plugin_component.metadata.sub_commands.is_empty() {
                if let Some((subcommand_name, args)) = self.args.subcommand() {
                    (
                        plugin_component
                            .metadata
                            .sub_commands
                            .iter()
                            .find(|sub_command| sub_command.name == subcommand_name)
                            .with_context(|| format!("no subcommand found for {name}"))?
                            .clone(),
                        args,
                    )
                } else {
                    bail!("no subcommand found for {name}")
                }
            } else {
                bail!("no command found for `{name}`");
            };

        trace!(command = ?run_command, args = ?run_args, "running command");

        for arg in run_command.arguments.iter_mut() {
            if let Ok(Some(value)) = run_args.try_get_one::<String>(arg.name.as_str()) {
                trace!(
                    ?arg.name,
                    ?value,
                    "found argument value in command matches, updating command argument",
                );
                arg.value = Some(value.to_owned());
            }
        }
        for (flag, arg) in run_command.flags.iter_mut() {
            if let Ok(Some(value)) = run_args.try_get_one::<String>(flag.as_str()) {
                trace!(
                    ?flag,
                    ?value,
                    "found flag value in command matches, updating command flag",
                );
                arg.value = Some(value.to_owned());
            }
        }

        // TODO(IMPORTANT): Only do this if the component has an HTTP export
        // TODO(IMPORTANT): get this or let user specify this?
        let listener = TcpListener::bind("127.0.0.1:8888").await?;

        let runtime_config = Arc::new(RwLock::new(HashMap::default()));
        tokio::spawn({
            let plugin_component = plugin_component.clone();
            let runtime_config = runtime_config.clone();
            async move {
                loop {
                    let runtime_config = runtime_config.clone();
                    let plugin_component = plugin_component.clone();
                    let (client, addr) = listener
                        .accept()
                        .await
                        .expect("failed to accept incoming request");
                    if let Err(e) = http1::Builder::new()
                        .keep_alive(true)
                        .serve_connection(
                            TokioIo::new(client),
                            hyper::service::service_fn(move |req| {
                                let runtime_config = runtime_config.clone();
                                let mut ctx = Ctx::builder();
                                if let Some(fs_root) = plugin_component.wasi_fs_root.as_ref() {
                                    ctx = ctx.with_wasi_ctx(
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
                                }

                                let mut store = plugin_component
                                    .component
                                    .new_store(ctx.with_runtime_config_arc(runtime_config).build());
                                let pre = plugin_component.component.instance_pre().to_owned();
                                async move {
                                    crate::cli::dev::handle_request(
                                        store.as_context_mut(),
                                        pre,
                                        req,
                                    )
                                    .await
                                }
                            }),
                        )
                        .await
                    {
                        error!(addr = ?addr, err = ?e, "error serving client");
                    }
                }
            }
        });

        // Instantiate and run plugin
        match plugin_component
            .call_run(Ctx::default(), &run_command, Arc::default())
            .await
        {
            Ok(res) => {
                debug!(name = ?name, "command executed");
                // Prepare output data for both text and JSON output
                let output_data = json!({
                    "command": run_command,
                    "plugin": plugin_component.metadata.name,
                    "output": res,
                    "success": true
                });

                Ok(CommandOutput::ok(res, Some(output_data)))
            }
            Err(e) => {
                debug!(name = ?name, "command executed with error ");
                // Prepare output data for both text and JSON output
                let output_data = json!({
                    "command": run_command,
                    "plugin": plugin_component.metadata.name,
                    "output": e.to_string(),
                    "success": false
                });
                Ok(CommandOutput::error(e, Some(output_data)))
            }
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
        let plugin_metadata: Vec<Metadata> = list_plugins(ctx.runtime(), ctx.data_dir())
            .await?
            .into_iter()
            .map(|p| p.metadata)
            .collect();

        match self.output {
            OutputKind::Text => {
                if plugin_metadata.is_empty() {
                    Ok(CommandOutput::ok("No plugins installed", None))
                } else {
                    let mut output = String::new();
                    output.push_str("Installed plugins:\n");
                    for plugin in &plugin_metadata {
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
                    "plugins": plugin_metadata,
                    "count": plugin_metadata.len()
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
            // TODO(GFI): support OCI references too
        };

        let mut output = String::new();

        let component =
            prepare_component_plugin(ctx.runtime(), &wasm, Some(ctx.data_dir().as_path())).await?;
        debug!(metadata = ?component.metadata, "plugin metadata");

        for name in &self.hooks {
            if let Some(hook) = component.metadata.hooks.iter().find(|h| h == &name) {
                match component
                    .call_hook(Ctx::default(), hook.to_owned(), Arc::default())
                    .await
                    .context("failed to run hook")
                {
                    Ok(out) => {
                        output.push_str(out.as_str());
                        output.push_str(&format!("Hook '{name}' executed successfully"));
                    }
                    Err(e) => {
                        warn!(err = ?e, name = %name, "Hook execution failed");
                        output.push_str(&format!("Hook '{name}' execution failed\n"));
                    }
                }
            } else {
                warn!(name = %name, "Hook not found in plugin metadata");
            }
        }

        // Handle the command if no hooks were executed
        let component = Arc::new(component);
        if output.is_empty() {
            // Prepend the name of the command to the args.
            // E.g. instead of wash plugin test ./foo.wasm foo bar
            //      make it    wash plugin test ./foo.wasm bar
            let mut args = Vec::with_capacity(self.args.len() + 1);
            args.push(&component.metadata().name);
            args.extend(self.args.iter());

            let cli_command: clap::Command = component.metadata().into();
            let matches = cli_command.get_matches_from(args);

            let component_plugin_command = ComponentPluginCommand {
                command_name: &component.metadata().name,
                args: &matches,
                plugin_component: Some(component.clone()),
            };

            output.push_str(component_plugin_command.handle(ctx).await?.message.as_str());
        }

        Ok(CommandOutput::ok(
            output,
            Some(json!({
                "plugin_path": self.plugin.to_string_lossy(),
                "args": self.args,
                "hooks": self.hooks,
                "metadata": component.metadata,
                "success": true
            })),
        ))
    }
}
