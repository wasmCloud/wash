use std::path::PathBuf;

use anyhow::Context;
use clap::{Args, Subcommand};
use etcetera::AppStrategy;
use serde_json::json;
use tokio::fs;
use tracing::{info, instrument, warn};

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
