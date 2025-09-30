//! Plugin management for wash
//!
//! This module provides functions for installing, uninstalling, and listing wash plugins.
//! Plugins are WebAssembly components that extend wash functionality.

use anyhow::{Context as _, bail};
use etcetera::AppStrategy;
use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument};
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
                wasmcloud::wash::types::{VolumeMount, VolumeMountPerms},
            },
        },
        plugin::Runner,
        prepare_component_plugin,
    },
};

/// Represents a resolved volume mount with actual host paths
#[derive(Debug, Clone)]
pub struct ResolvedVolumeMount {
    /// The resolved source path on the host filesystem
    pub source: PathBuf,
    /// The destination path in the guest filesystem
    pub destination: String,
    /// The permissions for this mount
    pub perms: VolumeMountPerms,
}

/// A [PluginComponent] represents a precompiled and linked WebAssembly component that
/// implements the wash plugin interface. It contains the component itself, its metadata,
/// and the resolved volume mounts for filesystem access using wasi:filesystem
pub struct PluginComponent {
    pub component: CustomCtxComponent<Ctx>,
    pub metadata: Metadata,
    /// Resolved volume mounts for this component. These are the actual host paths that will be
    /// mounted into the guest filesystem at the specified destinations.
    pub volume_mounts: Vec<ResolvedVolumeMount>,
    /// A read/write allowed directory for the component to use as its plugin-specific data directory
    pub wasi_fs_root: Option<PathBuf>,
}

impl std::fmt::Debug for PluginComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginComponent")
            .field("metadata", &self.metadata)
            .field("volume_mounts", &self.volume_mounts)
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

        // Resolve volume mounts from the metadata
        let volume_mounts = resolve_volume_mounts(&metadata.volume_mounts).await?;

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
            volume_mounts,
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
        // Create context with plugin-specific stdout/stderr streams
        let ctx_with_streams = Ctx::builder(self.metadata.name.clone())
            .with_wasi_ctx(ctx.ctx)
            .with_runtime_config_arc(ctx.runtime_config)
            .with_background_processes(ctx.background_processes)
            .build();
        let mut store = self.component.new_store(ctx_with_streams);
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
        // Create context with plugin-specific stdout/stderr streams
        let ctx_with_streams = Ctx::builder(self.metadata.name.clone())
            .with_wasi_ctx(ctx.ctx)
            .with_runtime_config_arc(ctx.runtime_config)
            .with_background_processes(ctx.background_processes)
            .build();
        let mut store = self.component.new_store(ctx_with_streams);
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
            .filter(|plugin| plugin.metadata.command.is_some())
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

/// Resolve volume mounts from plugin metadata into actual host paths
async fn resolve_volume_mounts(mounts: &[VolumeMount]) -> anyhow::Result<Vec<ResolvedVolumeMount>> {
    let mut resolved = Vec::new();

    for mount in mounts {
        let source = resolve_source_path(&mount.source).await?;

        // Validate that the source path exists
        if !source.exists() {
            bail!(
                "Volume mount source path '{}' does not exist (resolved from '{}')",
                source.display(),
                mount.source
            );
        }

        // Validate that the source is a directory
        if !source.is_dir() {
            bail!(
                "Volume mount source path '{}' is not a directory",
                source.display()
            );
        }

        resolved.push(ResolvedVolumeMount {
            source,
            destination: mount.destination.clone(),
            perms: mount.perms,
        });
    }

    Ok(resolved)
}

/// Resolve a source path string to an actual filesystem path
/// Handles special cases like "cwd", ".", and relative paths
async fn resolve_source_path(source: &str) -> anyhow::Result<PathBuf> {
    let path = match source {
        "cwd" | "." => env::current_dir().context("failed to get current working directory")?,
        _ if Path::new(source).is_absolute() => PathBuf::from(source),
        _ => {
            // Relative path - resolve relative to cwd
            let cwd = env::current_dir().context("failed to get current working directory")?;
            cwd.join(source)
        }
    };

    // Canonicalize the path to resolve any symlinks and get the absolute path
    match tokio::fs::canonicalize(&path).await {
        Ok(canonical) => Ok(canonical),
        Err(e) => {
            bail!(
                "Failed to resolve volume mount source path '{}': {}",
                source,
                e
            )
        }
    }
}

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
            let (component_data, _) = pull_component(&options.source, oci_config)
                .await
                .with_context(|| {
                    format!(
                        "failed to pull plugin from OCI registry: {}",
                        options.source
                    )
                })?;
            component_data
        };

    // Validate that it's a valid WebAssembly component and wash plugin
    let metadata = get_plugin_metadata(ctx.runtime(), &component_data).await?;

    // Validate that plugin commands don't conflict with built-in commands
    validate_plugin_commands(&metadata)?;

    // Validate that plugin commands don't conflict with existing plugins
    validate_plugin_conflicts(ctx, &metadata).await?;

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

        let Ok(component_plugin) =
            prepare_component_plugin(runtime, &plugin, Some(data_dir.as_ref())).await
        else {
            error!(plugin_name = %plugin_name, "failed to prepare plugin component, please uninstall, rebuild and reinstall the plugin");
            continue;
        };

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

/// Built-in wash commands that cannot be overridden by plugins
const BUILT_IN_COMMANDS: &[&str] = &[
    "build", "config", "dev", "doctor", "inspect", "new", "oci", "docker", // alias for oci
    "plugin", "update", "upgrade", // alias for update
];

/// Validate that plugin commands don't conflict with built-in commands
fn validate_plugin_commands(metadata: &Metadata) -> anyhow::Result<()> {
    // Check if plugin has a top-level command that conflicts
    if let Some(ref command) = metadata.command {
        let command_name = command.name.to_lowercase();
        if BUILT_IN_COMMANDS.contains(&command_name.as_str()) {
            bail!(
                "Plugin command '{}' conflicts with built-in wash command. \
                Built-in commands that cannot be overridden: {}",
                command.name,
                BUILT_IN_COMMANDS.join(", ")
            );
        }
    }

    // Check if plugin name itself conflicts (since plugins can be invoked as top-level commands)
    let plugin_name = metadata.name.to_lowercase();
    if BUILT_IN_COMMANDS.contains(&plugin_name.as_str()) {
        bail!(
            "Plugin name '{}' conflicts with built-in wash command. \
            Built-in commands that cannot be overridden: {}",
            metadata.name,
            BUILT_IN_COMMANDS.join(", ")
        );
    }

    // Check for subcommand conflicts within this plugin (same plugin, different subcommands with same name)
    let mut subcommand_names = std::collections::HashSet::new();
    for subcommand in &metadata.sub_commands {
        let sub_name = subcommand.name.to_lowercase();
        if !subcommand_names.insert(sub_name.clone()) {
            bail!(
                "Plugin '{}' has duplicate subcommand name '{}'. Each subcommand must have a unique name.",
                metadata.name,
                subcommand.name
            );
        }
    }

    Ok(())
}

/// Validate that plugin commands don't conflict with existing plugins
async fn validate_plugin_conflicts(
    ctx: &CliContext,
    new_metadata: &Metadata,
) -> anyhow::Result<()> {
    // Get all existing plugins
    let existing_plugins = list_plugins(ctx.runtime(), ctx.data_dir()).await?;

    let new_plugin_name = new_metadata.name.to_lowercase();

    for existing_plugin in existing_plugins {
        let existing_name = existing_plugin.metadata.name.to_lowercase();

        // Skip if this is the same plugin (for force installs)
        if existing_name == new_plugin_name {
            continue;
        }

        // Check for top-level command conflicts
        match (&new_metadata.command, &existing_plugin.metadata.command) {
            (Some(new_cmd), Some(existing_cmd)) => {
                let new_cmd_name = new_cmd.name.to_lowercase();
                let existing_cmd_name = existing_cmd.name.to_lowercase();

                if new_cmd_name == existing_cmd_name {
                    bail!(
                        "Plugin command '{}' conflicts with existing plugin '{}' command '{}'. \
                        Command names must be unique across all plugins.",
                        new_cmd.name,
                        existing_plugin.metadata.name,
                        existing_cmd.name
                    );
                }
            }
            (Some(new_cmd), None) => {
                // New plugin has command, existing plugin uses its name as command
                let new_cmd_name = new_cmd.name.to_lowercase();
                if new_cmd_name == existing_name {
                    bail!(
                        "Plugin command '{}' conflicts with existing plugin name '{}'. \
                        Plugin names and command names must be unique.",
                        new_cmd.name,
                        existing_plugin.metadata.name
                    );
                }
            }
            (None, Some(existing_cmd)) => {
                // New plugin uses its name as command, existing plugin has explicit command
                let existing_cmd_name = existing_cmd.name.to_lowercase();
                if new_plugin_name == existing_cmd_name {
                    bail!(
                        "Plugin name '{}' conflicts with existing plugin '{}' command '{}'. \
                        Plugin names and command names must be unique.",
                        new_metadata.name,
                        existing_plugin.metadata.name,
                        existing_cmd.name
                    );
                }
            }
            (None, None) => {
                // Both plugins use their names as commands - already handled by plugin name uniqueness
            }
        }

        // Check for subcommand conflicts (same plugin namespace)
        if let Some(ref new_cmd) = new_metadata.command
            && let Some(ref existing_cmd) = existing_plugin.metadata.command
        {
            let new_cmd_name = new_cmd.name.to_lowercase();
            let existing_cmd_name = existing_cmd.name.to_lowercase();

            // Only check subcommands if plugins have the same top-level command name
            if new_cmd_name == existing_cmd_name {
                for new_sub in &new_metadata.sub_commands {
                    for existing_sub in &existing_plugin.metadata.sub_commands {
                        let new_sub_name = new_sub.name.to_lowercase();
                        let existing_sub_name = existing_sub.name.to_lowercase();

                        if new_sub_name == existing_sub_name {
                            bail!(
                                "Plugin '{}' subcommand '{} {}' conflicts with existing plugin '{}' subcommand '{} {}'. \
                                    Subcommand names must be unique within the same command namespace.",
                                new_metadata.name,
                                new_cmd.name,
                                new_sub.name,
                                existing_plugin.metadata.name,
                                existing_cmd.name,
                                existing_sub.name
                            );
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::bindings::plugin::wasmcloud::wash::types::{Command, HookType};

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

    #[test]
    fn test_validate_plugin_commands_accepts_valid_names() {
        let metadata = Metadata {
            id: "test-plugin".to_string(),
            name: "test-plugin".to_string(),
            description: "A test plugin".to_string(),
            contact: "test@example.com".to_string(),
            url: "https://example.com".to_string(),
            license: "Apache-2.0".to_string(),
            version: "1.0.0".to_string(),
            command: Some(Command {
                id: "test".to_string(),
                name: "test".to_string(),
                description: "Test command".to_string(),
                flags: vec![],
                arguments: vec![],
                usage: vec![],
            }),
            sub_commands: vec![],
            hooks: vec![],
        };

        assert!(validate_plugin_commands(&metadata).is_ok());
    }

    #[test]
    fn test_validate_plugin_commands_rejects_conflicting_command_name() {
        let metadata = Metadata {
            id: "bad-plugin".to_string(),
            name: "bad-plugin".to_string(),
            description: "A bad plugin".to_string(),
            contact: "test@example.com".to_string(),
            url: "https://example.com".to_string(),
            license: "Apache-2.0".to_string(),
            version: "1.0.0".to_string(),
            command: Some(Command {
                id: "inspect".to_string(),
                name: "inspect".to_string(), // This conflicts with built-in command
                description: "Conflicting command".to_string(),
                flags: vec![],
                arguments: vec![],
                usage: vec![],
            }),
            sub_commands: vec![],
            hooks: vec![],
        };

        let result = validate_plugin_commands(&metadata);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("conflicts with built-in wash command")
        );
    }

    #[test]
    fn test_validate_plugin_commands_rejects_conflicting_plugin_name() {
        let metadata = Metadata {
            id: "build".to_string(),
            name: "build".to_string(), // This conflicts with built-in command
            description: "A bad plugin".to_string(),
            contact: "test@example.com".to_string(),
            url: "https://example.com".to_string(),
            license: "Apache-2.0".to_string(),
            version: "1.0.0".to_string(),
            command: None,
            sub_commands: vec![],
            hooks: vec![HookType::BeforeBuild],
        };

        let result = validate_plugin_commands(&metadata);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("conflicts with built-in wash command")
        );
    }

    #[test]
    fn test_validate_plugin_commands_case_insensitive() {
        let metadata = Metadata {
            id: "bad-plugin".to_string(),
            name: "bad-plugin".to_string(),
            description: "A bad plugin".to_string(),
            contact: "test@example.com".to_string(),
            url: "https://example.com".to_string(),
            license: "Apache-2.0".to_string(),
            version: "1.0.0".to_string(),
            command: Some(Command {
                id: "BUILD".to_string(),
                name: "BUILD".to_string(), // Uppercase should still conflict
                description: "Conflicting command".to_string(),
                flags: vec![],
                arguments: vec![],
                usage: vec![],
            }),
            sub_commands: vec![],
            hooks: vec![],
        };

        let result = validate_plugin_commands(&metadata);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("conflicts with built-in wash command")
        );
    }

    #[test]
    fn test_validate_plugin_commands_rejects_duplicate_subcommands() {
        let metadata = Metadata {
            id: "bad-plugin".to_string(),
            name: "bad-plugin".to_string(),
            description: "A bad plugin".to_string(),
            contact: "test@example.com".to_string(),
            url: "https://example.com".to_string(),
            license: "Apache-2.0".to_string(),
            version: "1.0.0".to_string(),
            command: None,
            sub_commands: vec![
                Command {
                    id: "sub1".to_string(),
                    name: "duplicate".to_string(),
                    description: "First subcommand".to_string(),
                    flags: vec![],
                    arguments: vec![],
                    usage: vec![],
                },
                Command {
                    id: "sub2".to_string(),
                    name: "duplicate".to_string(), // Same name as above
                    description: "Second subcommand".to_string(),
                    flags: vec![],
                    arguments: vec![],
                    usage: vec![],
                },
            ],
            hooks: vec![],
        };

        let result = validate_plugin_commands(&metadata);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("duplicate subcommand name")
        );
    }
}
