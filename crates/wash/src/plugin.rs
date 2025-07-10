//! Plugin management for wash
//!
//! This module provides functions for installing, uninstalling, and listing wash plugins.
//! Plugins are WebAssembly components that extend wash functionality.

use anyhow::{Context as _, bail};
use etcetera::AppStrategy;
use std::path::Path;
use tracing::{debug, info, instrument};

use crate::{
    cli::CliContext,
    oci::{OCI_CACHE_DIR, OciConfig, pull_component},
    runtime::{
        Ctx, bindings::plugin_host::wasmcloud::wash::plugin::Metadata, prepare_component_plugin,
    },
};

const PLUGINS_DIR: &str = "plugins";

#[derive(Debug, Clone)]
pub struct InstallPluginOptions {
    /// The source to install from (OCI reference or file path)
    pub source: String,
    /// Force overwrite if plugin already exists
    pub force: bool,
}

#[derive(Debug, Clone)]
pub struct InstallPluginResult {
    /// The name of the installed plugin
    pub name: String,
    /// The source from which the plugin was installed
    pub source: String,
    /// The path where the plugin was installed
    pub path: String,
    /// The size of the plugin file in bytes
    pub size: u64,
}

/// Install a plugin from an OCI reference or file path
#[instrument(level = "debug", skip_all, name = "install_plugin")]
pub async fn install_plugin(
    ctx: &CliContext,
    options: InstallPluginOptions,
) -> anyhow::Result<InstallPluginResult> {
    let plugins_dir = ctx.data_dir().join(PLUGINS_DIR);

    // Ensure plugins directory exists
    if !plugins_dir.exists() {
        tokio::fs::create_dir_all(&plugins_dir)
            .await
            .context("failed to create plugins directory")?;
    }

    // fetch plugin, call metadata, then write
    // Determine if source is OCI reference or file path
    let component_data =
        if options.source.starts_with("file://") || Path::new(&options.source).exists() {
            // Load from file
            let file_path = if options.source.starts_with("file://") {
                options.source.strip_prefix("file://").unwrap()
            } else {
                &options.source
            };

            debug!(path = %file_path, "loading plugin from file");
            tokio::fs::read(file_path)
                .await
                .with_context(|| format!("failed to read plugin file: {file_path}"))?
        } else {
            // Load from OCI registry
            debug!(reference = %options.source, "loading plugin from OCI registry");
            let oci_config = OciConfig::new_with_cache(ctx.cache_dir().join(OCI_CACHE_DIR));
            pull_component(&options.source, oci_config)
                .await
                .with_context(|| {
                    format!(
                        "failed to pull plugin from OCI registry: {}",
                        options.source
                    )
                })?
        };

    // Validate that it's a valid WebAssembly component and wash plugin
    let metadata = get_plugin_metadata(ctx, &component_data).await?;

    // Sanitize plugin name for filesystem storage
    let sanitized_name = sanitize_plugin_name(&metadata.name);
    let plugin_path = plugins_dir.join(format!("{}.wasm", sanitized_name));

    // Check if plugin already exists
    if plugin_path.exists() && !options.force {
        bail!(
            "Plugin '{}' already exists. Use --force option to overwrite",
            metadata.name
        );
    }
    // Write plugin to storage
    tokio::fs::write(&plugin_path, &component_data)
        .await
        .with_context(|| format!("failed to write plugin to: {}", plugin_path.display()))?;

    info!(
        name = %metadata.name,
        source = %options.source,
        path = %plugin_path.display(),
        "plugin installed successfully"
    );

    Ok(InstallPluginResult {
        name: metadata.name,
        source: options.source,
        path: plugin_path.display().to_string(),
        size: component_data.len() as u64,
    })
}

/// Uninstall a plugin
#[instrument(level = "debug", skip_all, name = "uninstall_plugin")]
pub async fn uninstall_plugin(ctx: &CliContext, name: &str) -> anyhow::Result<()> {
    let plugins_dir = ctx.data_dir().join(PLUGINS_DIR);
    let sanitized_name = sanitize_plugin_name(name);
    let plugin_path = plugins_dir.join(format!("{}.wasm", sanitized_name));

    // Check if plugin exists
    if !plugin_path.exists() {
        bail!("Plugin '{name}' is not installed");
    }

    // Remove plugin file
    tokio::fs::remove_file(&plugin_path)
        .await
        .with_context(|| format!("failed to remove plugin file: {}", plugin_path.display()))?;

    info!(
        name = %name,
        path = %plugin_path.display(),
        "plugin uninstalled successfully"
    );

    Ok(())
}

/// List all installed plugins, returning their bytes and [`Metadata`]
#[instrument(level = "debug", skip_all, name = "list_plugins")]
pub async fn list_plugins(ctx: &CliContext) -> anyhow::Result<Vec<(Vec<u8>, Metadata)>> {
    let plugins_dir = ctx.data_dir().join(PLUGINS_DIR);

    // If plugins directory doesn't exist, return empty list
    if !plugins_dir.exists() {
        return Ok(Vec::new());
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

        // Load plugin as Vec<u8>
        let plugin = tokio::fs::read(&path)
            .await
            .with_context(|| format!("failed to read plugin file: {}", path.display()))?;

        // Get plugin metadata using the guest call
        let metadata = get_plugin_metadata(ctx, &plugin)
            .await
            .with_context(|| format!("failed to get metadata for plugin: {}", plugin_name))?;

        plugins.push((plugin, metadata));
    }

    // Sort plugins by name
    plugins.sort_by(|(_, a), (_, b)| a.name.cmp(&b.name));

    Ok(plugins)
}

pub async fn get_plugin_metadata(ctx: &CliContext, wasm: &[u8]) -> anyhow::Result<Metadata> {
    // Load the wasm as a component
    let runtime = ctx.runtime();
    let component = prepare_component_plugin(runtime, wasm).await?;
    let pre = component.instance_pre();
    let mut store = component.new_store(Ctx::default());

    // Instantiate component
    let instance = pre
        .instantiate_async(&mut store)
        .await
        .context("failed to instantiate plugin")?;

    // Call the plugin host bindings to get metadata
    let plugin = crate::runtime::bindings::plugin_guest::PluginGuest::new(&mut store, &instance)
        .context("failed to create plugin host bindings")?;
    let metadata = plugin.wasmcloud_wash_plugin().call_info(&mut store).await?;

    Ok(metadata)
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
