use anyhow::Context as _;
use clap::{Args, Subcommand};
use etcetera::AppStrategy;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument};

use crate::{
    cli::{CliContext, CommandOutput, OutputKind},
    oci::{OCI_CACHE_DIR, OciConfig, pull_component, validate_component},
};

const PLUGINS_DIR: &str = "plugins";

#[derive(Subcommand, Debug, Clone)]
pub enum PluginCommand {
    /// Install a plugin from an OCI reference or file
    Install(InstallCommand),
    /// Uninstall a plugin
    Uninstall(UninstallCommand),
    /// List installed plugins
    List(ListCommand),
}

impl PluginCommand {
    /// Handle the plugin command
    #[instrument(level = "debug", skip_all, name = "plugin")]
    pub async fn handle(self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        match self {
            PluginCommand::Install(cmd) => cmd.handle(ctx).await,
            PluginCommand::Uninstall(cmd) => cmd.handle(ctx).await,
            PluginCommand::List(cmd) => cmd.handle(ctx).await,
        }
    }
}

#[derive(Args, Debug, Clone)]
pub struct InstallCommand {
    /// The plugin name to install
    #[clap(name = "name")]
    name: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMetadata {
    /// The name of the plugin
    pub name: String,
    /// The source from which the plugin was installed
    pub source: String,
    /// The installation timestamp
    pub installed_at: chrono::DateTime<chrono::Utc>,
    /// The size of the plugin file in bytes
    pub size: u64,
}

impl InstallCommand {
    /// Handle the plugin install command
    #[instrument(level = "debug", skip_all, name = "plugin_install")]
    pub async fn handle(self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let plugins_dir = ctx.data_dir().join(PLUGINS_DIR);

        // Ensure plugins directory exists
        if !plugins_dir.exists() {
            tokio::fs::create_dir_all(&plugins_dir)
                .await
                .context("failed to create plugins directory")?;
        }

        // Sanitize plugin name for filesystem storage
        let sanitized_name = sanitize_plugin_name(&self.name);
        let plugin_path = plugins_dir.join(format!("{}.wasm", sanitized_name));
        let metadata_path = plugins_dir.join(format!("{}.meta.json", sanitized_name));

        // Check if plugin already exists
        if plugin_path.exists() && !self.force {
            return Ok(CommandOutput::error(
                format!(
                    "Plugin '{}' already exists. Use --force to overwrite.",
                    self.name
                ),
                None,
            ));
        }

        // Determine if source is OCI reference or file path
        let component_data =
            if self.source.starts_with("file://") || std::path::Path::new(&self.source).exists() {
                // Load from file
                let file_path = if self.source.starts_with("file://") {
                    self.source.strip_prefix("file://").unwrap()
                } else {
                    &self.source
                };

                debug!(path = %file_path, "loading plugin from file");
                tokio::fs::read(file_path)
                    .await
                    .with_context(|| format!("failed to read plugin file: {}", file_path))?
            } else {
                // Load from OCI registry
                debug!(reference = %self.source, "loading plugin from OCI registry");
                let oci_config = OciConfig::new_with_cache(ctx.cache_dir().join(OCI_CACHE_DIR));
                pull_component(&self.source, oci_config)
                    .await
                    .with_context(|| {
                        format!("failed to pull plugin from OCI registry: {}", self.source)
                    })?
            };

        // Validate that it's a valid WebAssembly component
        validate_component(&component_data)
            .with_context(|| format!("invalid WebAssembly component: {}", self.source))?;

        // Write plugin to storage
        tokio::fs::write(&plugin_path, &component_data)
            .await
            .with_context(|| format!("failed to write plugin to: {}", plugin_path.display()))?;

        // Create metadata
        let metadata = PluginMetadata {
            name: self.name.clone(),
            source: self.source.clone(),
            installed_at: chrono::Utc::now(),
            size: component_data.len() as u64,
        };

        // Write metadata
        let metadata_json = serde_json::to_string_pretty(&metadata)
            .context("failed to serialize plugin metadata")?;
        tokio::fs::write(&metadata_path, metadata_json)
            .await
            .with_context(|| {
                format!(
                    "failed to write plugin metadata to: {}",
                    metadata_path.display()
                )
            })?;

        info!(
            name = %self.name,
            source = %self.source,
            path = %plugin_path.display(),
            "plugin installed successfully"
        );

        Ok(CommandOutput::ok(
            format!(
                "Plugin '{}' installed successfully from '{}'",
                self.name, self.source
            ),
            Some(serde_json::json!({
                "name": self.name,
                "source": self.source,
                "path": plugin_path.display().to_string(),
                "size": component_data.len(),
                "success": true
            })),
        ))
    }
}

impl UninstallCommand {
    /// Handle the plugin uninstall command
    #[instrument(level = "debug", skip_all, name = "plugin_uninstall")]
    pub async fn handle(self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let plugins_dir = ctx.data_dir().join(PLUGINS_DIR);
        let sanitized_name = sanitize_plugin_name(&self.name);
        let plugin_path = plugins_dir.join(format!("{}.wasm", sanitized_name));
        let metadata_path = plugins_dir.join(format!("{}.meta.json", sanitized_name));

        // Check if plugin exists
        if !plugin_path.exists() {
            return Ok(CommandOutput::error(
                format!("Plugin '{}' is not installed", self.name),
                None,
            ));
        }

        // Remove plugin file
        tokio::fs::remove_file(&plugin_path)
            .await
            .with_context(|| format!("failed to remove plugin file: {}", plugin_path.display()))?;

        // Remove metadata file (if it exists)
        if metadata_path.exists() {
            tokio::fs::remove_file(&metadata_path)
                .await
                .with_context(|| {
                    format!(
                        "failed to remove plugin metadata: {}",
                        metadata_path.display()
                    )
                })?;
        }

        info!(
            name = %self.name,
            path = %plugin_path.display(),
            "plugin uninstalled successfully"
        );

        Ok(CommandOutput::ok(
            format!("Plugin '{}' uninstalled successfully", self.name),
            Some(serde_json::json!({
                "name": self.name,
                "success": true
            })),
        ))
    }
}

impl ListCommand {
    /// Handle the plugin list command
    #[instrument(level = "debug", skip_all, name = "plugin_list")]
    pub async fn handle(self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let plugins_dir = ctx.data_dir().join(PLUGINS_DIR);

        // If plugins directory doesn't exist, return empty list
        if !plugins_dir.exists() {
            return Ok(match self.output {
                OutputKind::Text => CommandOutput::ok("No plugins installed", None),
                OutputKind::Json => CommandOutput::ok(
                    "",
                    Some(serde_json::json!({
                        "plugins": [],
                        "count": 0
                    })),
                ),
            });
        }

        // Read directory contents
        let mut entries = tokio::fs::read_dir(&plugins_dir)
            .await
            .context("failed to read plugins directory")?;

        let mut plugins = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            // Only process .wasm files
            if path.extension().and_then(|ext| ext.to_str()) != Some("wasm") {
                continue;
            }

            let plugin_name = path
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("unknown");

            // Try to read metadata
            let metadata_path = plugins_dir.join(format!("{}.meta.json", plugin_name));
            let metadata = if metadata_path.exists() {
                match tokio::fs::read_to_string(&metadata_path).await {
                    Ok(content) => match serde_json::from_str::<PluginMetadata>(&content) {
                        Ok(meta) => Some(meta),
                        Err(e) => {
                            debug!(path = %metadata_path.display(), error = %e, "failed to parse plugin metadata");
                            None
                        }
                    },
                    Err(e) => {
                        debug!(path = %metadata_path.display(), error = %e, "failed to read plugin metadata");
                        None
                    }
                }
            } else {
                None
            };

            // Get file size
            let file_size = entry.metadata().await?.len();

            let plugin_info = if let Some(meta) = metadata {
                meta
            } else {
                // Create minimal metadata from available information
                PluginMetadata {
                    name: plugin_name.to_string(),
                    source: "unknown".to_string(),
                    installed_at: chrono::DateTime::from_timestamp(0, 0)
                        .unwrap_or_else(chrono::Utc::now),
                    size: file_size,
                }
            };

            plugins.push(plugin_info);
        }

        // Sort plugins by name
        plugins.sort_by(|a, b| a.name.cmp(&b.name));

        match self.output {
            OutputKind::Text => {
                if plugins.is_empty() {
                    Ok(CommandOutput::ok("No plugins installed", None))
                } else {
                    let mut output = String::new();
                    output.push_str("Installed plugins:\n");
                    for plugin in &plugins {
                        output.push_str(&format!(
                            "  {} ({})\n    Source: {}\n    Size: {} bytes\n    Installed: {}\n",
                            plugin.name,
                            plugin.name,
                            plugin.source,
                            plugin.size,
                            plugin.installed_at.format("%Y-%m-%d %H:%M:%S UTC")
                        ));
                    }
                    Ok(CommandOutput::ok(output.trim_end(), None))
                }
            }
            OutputKind::Json => Ok(CommandOutput::ok(
                "",
                Some(serde_json::json!({
                    "plugins": plugins,
                    "count": plugins.len()
                })),
            )),
        }
    }
}

/// Sanitize a plugin name for filesystem storage
///
/// This function removes or replaces characters that are not safe for use in filenames
/// across different operating systems.
fn sanitize_plugin_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => c,
            _ => '_',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_plugin_name() {
        assert_eq!(sanitize_plugin_name("hello-world"), "hello-world");
        assert_eq!(sanitize_plugin_name("hello_world"), "hello_world");
        assert_eq!(sanitize_plugin_name("hello/world"), "hello_world");
        assert_eq!(sanitize_plugin_name("hello:world"), "hello_world");
        assert_eq!(sanitize_plugin_name("hello@world"), "hello_world");
        assert_eq!(sanitize_plugin_name("hello world"), "hello_world");
        assert_eq!(sanitize_plugin_name("hello.world"), "hello_world");
        assert_eq!(sanitize_plugin_name("123-abc_XYZ"), "123-abc_XYZ");
    }
}
