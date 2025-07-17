use std::path::PathBuf;

use anyhow::Context;
use clap::{Args, Subcommand};
use etcetera::AppStrategy;
use serde_json::json;
use tokio::fs;
use tracing::{debug, info, instrument, trace, warn};

use crate::{
    cli::{CliCommand, CliContext, CommandOutput, OutputKind, component_build::build_component},
    config::Config,
    plugin::{InstallPluginOptions, install_plugin, list_plugins, uninstall_plugin},
    runtime::{
        Ctx, bindings::plugin_guest::PluginGuest,
        bindings::plugin_guest::exports::wasmcloud::wash::plugin::HookType,
        prepare_component_plugin, types::Runner,
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
    /// Test a plugin component for its metadata, commands or hooks
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
pub type ComponentPluginCommand = Vec<String>;

/// This implementation allows for arguments parsed by clap to locate and execute a plugin command.
impl CliCommand for &ComponentPluginCommand {
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let command_name = self.first().context("no command provided to plugin")?;

        // Find the plugin component where its metadata name matches the command
        let Some(plugin_component) = ctx
            .plugin_manager()
            .get_commands()
            .into_iter()
            .find(|plugin| &plugin.metadata.name == command_name)
        else {
            anyhow::bail!("no plugin command found for `{command_name}`");
        };

        let name = &plugin_component.metadata.name;
        let mut cli_command = clap::Command::new(name)
            .about(&plugin_component.metadata.description)
            .allow_hyphen_values(true)
            .disable_help_flag(false)
            .subcommand_required(false);
        let mut command = plugin_component
            .metadata
            .commands
            .iter()
            .find(|c| &c.name == name)
            .context(format!("command `{name}` not found in plugin metadata"))?
            .to_owned();

        // Populate the CLI command with args and flags
        for argument in &command.arguments {
            cli_command = cli_command.arg(argument)
        }
        for (flag, argument) in &command.flags {
            cli_command = cli_command.arg(Into::<clap::Arg>::into(argument).long(flag))
        }

        // Print help if no args are provided or if --help or -h is used
        if self.len() == 1
            || self.contains(&"--help".to_string())
            || self.contains(&"-h".to_string())
        {
            return cli_command
                .print_help()
                .map(|()| CommandOutput::ok("", None))
                .context("failed to print help for command");
        }

        // Populate command args with user-provided arguments
        let cli_args = cli_command.clone().get_matches_from(*self);
        for arg in command.arguments.iter_mut() {
            if let Ok(Some(value)) = cli_args.try_get_one::<String>(arg.name.as_str()) {
                trace!(
                    ?arg.name,
                    ?value,
                    "found argument value in command matches, updating command argument",
                );
                arg.value = value.to_owned();
            }
        }
        for (flag, arg) in command.flags.iter_mut() {
            if let Ok(Some(value)) = cli_args.try_get_one::<String>(flag.as_str()) {
                trace!(
                    ?flag,
                    ?value,
                    "found flag value in command matches, updating command flag",
                );
                arg.value = value.to_owned();
            }
        }

        let mut store = plugin_component.component.new_store(Ctx::default());
        let instance = plugin_component
            .component
            .instance_pre()
            .instantiate_async(&mut store)
            .await?;
        let plugin_guest = PluginGuest::new(&mut store, &instance)?;
        let guest = plugin_guest.wasmcloud_wash_plugin();
        let runner = store
            .data_mut()
            .table
            .push(Runner::new(plugin_component.metadata.clone()))?;
        guest
            .call_run(store, runner, &command)
            .await
            .context(format!("failed to run command `{name}`"))?
            .map_err(|()| anyhow::anyhow!("command `{name}` execution failed"))?;
        debug!(name = ?name, "command executed");

        // Prepare output data for both text and JSON output
        let output_data = json!({
            "command": name,
            "args": self,
            "plugin": plugin_component.metadata.name,
            "description": command.description,
            "success": true
        });

        // Compose a human-readable message
        let message = format!(
            "Successfully executed plugin command `{}` (plugin: `{}`)",
            name, plugin_component.metadata.name
        );

        Ok(CommandOutput::ok(message, Some(output_data)))
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
    /// The command names to test
    #[clap(name = "name", long = "command")]
    pub command: Vec<String>,
    /// The hook types to test
    #[clap(name = "type", long = "hook")]
    pub hooks: Vec<HookType>,
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
                format!("Failed to uninstall plugin '{}': {}", self.name, e),
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
                        let detail = if plugin.hooks.as_ref().is_some_and(|h| !h.is_empty()) {
                            format!(
                                "Hooks: {}\n",
                                plugin
                                    .hooks
                                    .as_ref()
                                    // SAFETY: We just checked that this is Some above
                                    .expect("hooks to exist")
                                    .iter()
                                    .map(|h| h.to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        } else if !&plugin.commands.is_empty() {
                            format!(
                                "Commands: {}\n",
                                plugin
                                    .commands
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
            // TODO: load config from a real place
            let built_path = build_component(&self.plugin, ctx, &Config::default())
                .await
                .context("Failed to build component from directory")?;
            fs::read(&built_path.artifact_path)
                .await
                .context("Failed to read built component file")?
        } else {
            fs::read(&self.plugin)
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
            PluginGuest::new(&mut store, &instance).context("Failed to get plugin_guest export")?;
        let guest = plugin_guest.wasmcloud_wash_plugin();

        let metadata = guest.call_info(&mut store).await?;
        info!(metadata = ?metadata, "plugin metadata");

        for name in &self.command {
            if let Some(command) = metadata.commands.iter().find(|c| c.name == *name) {
                let runner = store.data_mut().table.push(Runner::new(metadata.clone()))?;
                match guest
                    .call_run(&mut store, runner, command)
                    .await
                    .context("failed to run command")?
                {
                    Ok(()) => {
                        output.push_str(&format!("Command '{name}' executed successfully"));
                    }
                    Err(()) => {
                        warn!(name = %name, "Command execution failed");
                        output.push_str(&format!("Command '{name}' execution failed\n"));
                    }
                }
            } else {
                warn!(name = %name, "Command not found in plugin metadata");
            }
        }

        for name in &self.hooks {
            if let Some(hook) = metadata
                .hooks
                .iter()
                .find_map(|hooks_vec| hooks_vec.iter().find(|h| h == &name))
            {
                let runner = store.data_mut().table.push(Runner::new(metadata.clone()))?;
                match guest
                    .call_hook(&mut store, runner, hook.to_owned())
                    .await
                    .context("failed to run hook")?
                {
                    Ok(()) => {
                        output.push_str(&format!("Hook '{name}' executed successfully"));
                    }
                    Err(()) => {
                        warn!(name = %name, "Hook execution failed");
                        output.push_str(&format!("Hook '{name}' execution failed\n"));
                    }
                }
            } else {
                warn!(name = %name, "Hook not found in plugin metadata");
            }
        }

        Ok(CommandOutput::ok(
            output,
            Some(json!({
                "plugin_path": self.plugin.to_string_lossy(),
                "command": self.command,
                "hooks": self.hooks,
                "metadata": metadata,
                "success": true
            })),
        ))
    }
}
