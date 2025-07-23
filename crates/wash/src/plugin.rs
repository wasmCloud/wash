//! Plugin management for wash
//!
//! This module provides functions for installing, uninstalling, and listing wash plugins.
//! Plugins are WebAssembly components that extend wash functionality.

use anyhow::{Context as _, bail};
use etcetera::AppStrategy;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::RwLock;
use tracing::{debug, info, instrument};
use wasmcloud_runtime::{Runtime, component::CustomCtxComponent};

use crate::{
    cli::CliContext,
    oci::{OCI_CACHE_DIR, OciConfig, pull_component},
    runtime::{
        Ctx,
        bindings::{
            self,
            plugin::{
                WashPlugin,
                exports::wasmcloud::wash::plugin::{HookType, Metadata},
            },
        },
        plugin::Runner,
        prepare_component_plugin,
    },
};

/// A [PluginComponent] represents a precompiled and linked WebAssembly component that
/// implements the wash plugin interface. It contains the component itself, its metadata,
/// and the filesystem root where the plugin can read and write files using wasi:filesystem
pub struct PluginComponent {
    pub component: CustomCtxComponent<Ctx>,
    pub metadata: Metadata,
    /// A read/write allowed directory for the component to use as its filesystem root
    pub wasi_fs_root: Option<PathBuf>,
}

impl std::fmt::Debug for PluginComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginComponent")
            .field("metadata", &self.metadata)
            .field("wasi_fs_root", &self.wasi_fs_root)
            .finish()
    }
}

impl PluginComponent {
    pub async fn new(
        component: CustomCtxComponent<Ctx>,
        data_dir: Option<impl AsRef<Path>>,
    ) -> anyhow::Result<Self> {
        let pre = component.instance_pre();
        let mut store = component.new_store(Ctx::default());
        // Instantiate component
        let instance = pre
            .instantiate_async(&mut store)
            .await
            .context("failed to instantiate plugin")?;
        // Call the plugin host bindings to get metadata. If this succeeds, we know that
        // the component is a valid wash plugin.
        let plugin = crate::runtime::bindings::plugin::WashPlugin::new(&mut store, &instance)
            .context("failed to create plugin host bindings")?;
        let metadata = plugin.wasmcloud_wash_plugin().call_info(&mut store).await?;

        // Determine the filesystem root based on the plugin name
        let wasi_fs_root = data_dir.map(|dir| {
            dir.as_ref()
                .join("plugins")
                .join("fs")
                .join(sanitize_plugin_name(&metadata.name))
        });

        if let Some(ref fs_root) = wasi_fs_root {
            // Ensure the filesystem root directory exists
            tokio::fs::create_dir_all(fs_root)
                .await
                .context("failed to create plugin filesystem root directory")?;
        }

        Ok(Self {
            component,
            metadata,
            wasi_fs_root,
        })
    }

    /// Call the plugin's `call_info` method to retrieve its metadata. Only use this
    /// method if you need to get metadata without instantiating the component. Otherwise,
    /// [`PluginComponent::metadata`] already contains the metadata.
    pub async fn call_info(&self, ctx: Ctx) -> anyhow::Result<Metadata> {
        // Create a new store with the default context
        let mut store = self.component.new_store(ctx);
        // Instantiate the component and call the plugin host bindings to get metadata
        let instance = self
            .component
            .instance_pre()
            .instantiate_async(&mut store)
            .await
            .context("failed to instantiate plugin")?;
        // Get bindings
        let plugin = crate::runtime::bindings::plugin::WashPlugin::new(&mut store, &instance)
            .context("failed to create plugin host bindings")?;
        plugin.wasmcloud_wash_plugin().call_info(&mut store).await
    }

    /// Instantiate a new instance of this component and call a specific hook
    /// with the provided context
    pub async fn call_hook(
        &self,
        ctx: Ctx,
        hook: HookType,
        runner_context: Arc<RwLock<HashMap<String, String>>>,
    ) -> anyhow::Result<String> {
        let mut store = self.component.new_store(ctx);
        let instance = self
            .component
            .instance_pre()
            .instantiate_async(&mut store)
            .await
            .context("failed to instantiate plugin")?;
        let plugin = WashPlugin::new(&mut store, &instance)
            .context("failed to create plugin host bindings")?;

        // We'll use the same runner for initialization and the hook
        let runner = Runner::new(self.metadata.clone(), runner_context);

        let initialize_runner = store.data_mut().table.push(runner.clone())?;
        plugin
            .wasmcloud_wash_plugin()
            .call_initialize(&mut store, initialize_runner)
            .await?
            .map_err(|e| anyhow::anyhow!(e))?;

        let hook_runner = store.data_mut().table.push(runner)?;
        // Call the hook with the provided runner
        plugin
            .wasmcloud_wash_plugin()
            .call_hook(&mut store, hook_runner, hook)
            .await?
            .map_err(|e| anyhow::anyhow!(e))
    }

    /// Instantiate a new instance of this component and call the run function
    pub async fn call_run(
        &self,
        ctx: Ctx,
        command: &bindings::plugin::wasmcloud::wash::types::Command,
        runner_context: Arc<RwLock<HashMap<String, String>>>,
    ) -> anyhow::Result<String> {
        let mut store = self.component.new_store(ctx);
        let instance = self
            .component
            .instance_pre()
            .instantiate_async(&mut store)
            .await
            .context("failed to instantiate plugin")?;
        let plugin = WashPlugin::new(&mut store, &instance)
            .context("failed to create plugin host bindings")?;
        let runner = Runner::new(self.metadata.clone(), runner_context);

        let initialize_runner = store.data_mut().table.push(runner.clone())?;
        plugin
            .wasmcloud_wash_plugin()
            .call_initialize(&mut store, initialize_runner)
            .await?
            .map_err(|e| anyhow::anyhow!(e))?;

        let command_runner = store.data_mut().table.push(runner)?;

        // Call the hook with the provided runner
        plugin
            .wasmcloud_wash_plugin()
            .call_run(&mut store, command_runner, command)
            .await?
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
}

/// A struct responsible for managing Wash plugins
#[derive(Debug)]
pub struct PluginManager {
    pub plugins: Vec<Arc<PluginComponent>>,
}

impl PluginManager {
    pub async fn initialize(runtime: &Runtime, data_dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let plugins = list_plugins(runtime, data_dir.as_ref())
            .await?
            .into_iter()
            .map(Arc::new)
            .collect();
        Ok(Self { plugins })
    }

    /// Filter plugins that implement the given hook type
    pub fn get_hooks(&self, hook_type: HookType) -> Vec<Arc<PluginComponent>> {
        self.plugins
            .clone()
            .into_iter()
            .filter(|plugin| plugin.metadata.hooks.contains(&hook_type))
            .collect()
    }

    /// Get all plugins that implement top level commands
    pub fn get_commands(&self) -> Vec<Arc<PluginComponent>> {
        self.plugins
            .clone()
            .into_iter()
            .filter(|plugin| plugin.metadata.command.is_none())
            .collect()
    }

    /// Get the component for a specific top level command
    pub fn get_command(&self, plugin_name: &str) -> Option<Arc<PluginComponent>> {
        self.plugins
            .clone()
            .into_iter()
            .find(|plugin| plugin.metadata.name == plugin_name && plugin.metadata.command.is_some())
    }

    /// Get the component for a specific subcommand
    pub fn get_subcommand(
        &self,
        plugin_name: &str,
        subcommand: &str,
    ) -> Option<Arc<PluginComponent>> {
        self.plugins.clone().into_iter().find(|plugin| {
            plugin.metadata.name == plugin_name
                && plugin
                    .metadata
                    .sub_commands
                    .iter()
                    .any(|cmd| cmd.name == subcommand)
        })
    }

    /// Get all registered plugins
    pub fn get_plugins(&self) -> &[Arc<PluginComponent>] {
        &self.plugins
    }
}

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
    let metadata = get_plugin_metadata(ctx.runtime(), &component_data).await?;

    // Sanitize plugin name for filesystem storage
    let sanitized_name = sanitize_plugin_name(&metadata.name);
    let plugin_path = plugins_dir.join(format!("{sanitized_name}.wasm"));

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
    let plugin_path = plugins_dir.join(format!("{sanitized_name}.wasm"));

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
pub async fn list_plugins(
    runtime: &Runtime,
    data_dir: impl AsRef<Path>,
) -> anyhow::Result<Vec<PluginComponent>> {
    let plugins_dir = data_dir.as_ref().join(PLUGINS_DIR);

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

        let component_plugin = prepare_component_plugin(runtime, &plugin, Some(data_dir.as_ref()))
            .await
            .with_context(|| format!("failed to prepare plugin component: {plugin_name}"))?;

        plugins.push(component_plugin);
    }

    // Sort plugins by name
    plugins.sort_by(|a, b| a.metadata.name.cmp(&b.metadata.name));

    Ok(plugins)
}

/// Get metadata for a plugin from its WebAssembly bytes. This should only be used if you
/// don't need to use the plugin component again, otherwise prefer to use [`prepare_component_plugin`] directly.
pub async fn get_plugin_metadata(runtime: &Runtime, wasm: &[u8]) -> anyhow::Result<Metadata> {
    Ok(prepare_component_plugin(runtime, wasm, None)
        .await?
        .metadata)
}

/// Sanitize a plugin name for filesystem storage
///
/// This function removes or replaces characters that are not safe for use in filenames
/// across different operating systems.
pub(crate) fn sanitize_plugin_name(name: &str) -> String {
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
