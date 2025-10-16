//! Plugin management for wash
//!
//! This module provides functions for installing, uninstalling, and listing wash plugins.
//! Plugins are WebAssembly components that extend wash functionality.

use crate::{
    cli::{CliContext, oci::OCI_CACHE_DIR},
    plugin::{
        bindings::{
            WashPlugin,
            wasmcloud::wash::types::{HookType, Metadata},
        },
        runner::Runner,
    },
};
use anyhow::{Context as _, bail};
use bytes::Bytes;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::RwLock;
use tracing::{debug, error, info, instrument};
use wasmcloud::{
    engine::workload::{ResolvedWorkload, WorkloadComponent},
    host::HostApi,
    oci::{OciConfig, pull_component},
    plugin::HostPlugin,
    types::{Component, LocalResources, Workload, WorkloadStartRequest, WorkloadState},
    wit::{WitInterface, WitWorld},
};

pub mod bindings;
pub mod runner;
pub mod tracing_streams;

pub(crate) const PLUGIN_MANAGER_ID: &str = "wash-plugin-manager";
const PLUGINS_DIR: &str = "plugins";
const PLUGINS_FS_DIR: &str = "fs";

#[derive(Debug, Default)]
pub struct PluginManager {
    /// All registered plugins
    plugins: Arc<RwLock<Vec<Arc<PluginComponent>>>>,
    /// Optional directory where plugins can store data
    data_dir: Option<PathBuf>,
    /// A map of configuration from workload id to key-value pairs
    runtime_config: Arc<RwLock<HashMap<String, HashMap<String, String>>>>,
    /// Whether to skip confirmation prompts for host exec operations
    skip_confirmation: bool,
}

impl PluginManager {
    /// Create a new PluginManager
    pub fn new(skip_confirmation: bool) -> Self {
        Self {
            plugins: Arc::default(),
            data_dir: None,
            runtime_config: Arc::default(),
            skip_confirmation,
        }
    }

    /// Load existing plugins from the plugins directory and start their workloads
    pub async fn load_plugins(
        &self,
        ctx: &CliContext,
        data_dir: impl AsRef<Path>,
    ) -> anyhow::Result<()> {
        let plugins_dir = data_dir.as_ref().join(PLUGINS_DIR);

        // If plugins directory doesn't exist, create it
        if !plugins_dir.exists() {
            tokio::fs::create_dir_all(&plugins_dir)
                .await
                .context("failed to create plugins directory")?;
        }

        // Read directory contents
        let mut entries = tokio::fs::read_dir(&plugins_dir)
            .await
            .context("failed to read plugins directory")?;

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

            let workload = Workload {
                namespace: "plugins".to_string(),
                name: plugin_name.to_string(),
                annotations: HashMap::new(),
                service: None,
                components: vec![Component {
                    bytes: plugin.into(),
                    local_resources: LocalResources::default(),
                    pool_size: 1,
                    max_invocations: 1,
                }],
                host_interfaces: vec![
                    WitInterface::from("wasmcloud:wash/types@0.0.2"),
                    WitInterface::from("wasi:config/runtime@0.2.0-draft"),
                ],
                volumes: vec![],
            };

            let res = ctx
                .host()
                .workload_start(WorkloadStartRequest { workload })
                .await?;
            if res.workload_status.workload_state != WorkloadState::Running {
                error!(
                    name = %plugin_name,
                    state = ?res.workload_status.workload_state,
                    "failed to start plugin workload"
                );
                continue;
            }
        }

        Ok(())
    }

    /// Filter plugins that implement the given hook type
    pub async fn get_hooks(&self, hook_type: HookType) -> Vec<Arc<PluginComponent>> {
        self.plugins
            .read()
            .await
            .clone()
            .into_iter()
            .filter(|plugin| plugin.metadata.hooks.contains(&hook_type))
            .collect()
    }

    /// Get all plugins that implement top level commands
    pub async fn get_commands(&self) -> Vec<Arc<PluginComponent>> {
        self.plugins
            .read()
            .await
            .clone()
            .into_iter()
            .filter(|plugin| plugin.metadata.command.is_some())
            .collect()
    }

    /// Get the component for a specific top level command
    pub async fn get_command(&self, plugin_name: &str) -> Option<Arc<PluginComponent>> {
        self.plugins
            .read()
            .await
            .clone()
            .into_iter()
            .find(|plugin| plugin.metadata.name == plugin_name && plugin.metadata.command.is_some())
    }

    /// Get the component for a specific subcommand
    pub async fn get_subcommand(
        &self,
        plugin_name: &str,
        subcommand: &str,
    ) -> Option<Arc<PluginComponent>> {
        self.plugins
            .read()
            .await
            .clone()
            .into_iter()
            .find(|plugin| {
                plugin.metadata.name == plugin_name
                    && plugin
                        .metadata
                        .sub_commands
                        .iter()
                        .any(|cmd| cmd.name == subcommand)
            })
    }

    /// Get all registered plugins
    pub async fn get_plugins(&self) -> Vec<Arc<PluginComponent>> {
        self.plugins.read().await.clone()
    }

    /// Return the [`PluginComponent`] associated with the given workload_id, if it
    /// exists. Useful for retrieving a newly installed plugin
    pub async fn get_plugin_by_workload_id(
        &self,
        workload_id: impl AsRef<str>,
    ) -> Option<Arc<PluginComponent>> {
        let workload_id = workload_id.as_ref();
        self.plugins
            .read()
            .await
            .iter()
            .find(|p| p.workload.id() == workload_id)
            .cloned()
    }

    pub fn skip_confirmation(&self) -> bool {
        self.skip_confirmation
    }
}

#[async_trait::async_trait]
impl HostPlugin for PluginManager {
    fn id(&self) -> &'static str {
        PLUGIN_MANAGER_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            // The plugin manager provides (imports from components' perspective) the types interface
            imports: HashSet::from([WitInterface::from("wasmcloud:wash/types@0.0.2")]),
            exports: HashSet::new(),
        }
    }

    async fn on_component_bind(
        &self,
        component: &mut WorkloadComponent,
        interfaces: HashSet<WitInterface>,
    ) -> anyhow::Result<()> {
        // Should only be asking for `wasmcloud:wash/types`
        if interfaces.len() != 1 {
            bail!("component tried to bind to plugin host with unsupported interfaces");
        }
        if let Some(interface) = interfaces.iter().next()
            && (interface.namespace != "wasmcloud"
                || interface.package != "wash"
                || !interface.interfaces.contains("types"))
        {
            bail!("component tried to bind to plugin host with unsupported interface: {interface}");
        }
        info!(
            "PluginManager binding to component with interfaces {}",
            component.local_resources().cpu_limit
        );

        // Add the types interface (provides runner, context, etc. to components that import wasmcloud:wash/types)
        bindings::wasmcloud::wash::types::add_to_linker(component.linker(), |ctx| ctx)?;

        Ok(())
    }

    async fn on_workload_resolved(
        &self,
        workload: &ResolvedWorkload,
        component_id: &str,
    ) -> anyhow::Result<()> {
        debug!("installing plugin");
        // TODO: what's the best way to do this? maybe a label
        let skip_confirmation = false;
        let plugin = PluginComponent::new(
            workload,
            component_id,
            self.data_dir.as_ref(),
            skip_confirmation,
        )
        .await?;

        self.plugins.write().await.push(Arc::new(plugin));

        Ok(())
    }
}

/// A [PluginComponent] represents a precompiled and linked WebAssembly component that
/// implements the wash plugin interface. It contains the component itself, its metadata,
/// and the filesystem root where the plugin can read and write files using wasi:filesystem
pub struct PluginComponent {
    /// The live component ID of the plugin
    pub id: String,
    pub workload: ResolvedWorkload,
    pub metadata: Metadata,
    /// A read/write allowed directory for the component to use as its filesystem root
    pub wasi_fs_root: Option<PathBuf>,
    /// Whether or not to skip confirmation prompts for host exec operations
    pub(crate) skip_confirmation: bool,
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
        workload: &ResolvedWorkload,
        component_id: &str,
        data_dir: Option<impl AsRef<Path>>,
        skip_confirmation: bool,
    ) -> anyhow::Result<Self> {
        let pre = workload.instantiate_pre(component_id).await?;
        let mut store = workload.new_store(component_id).await?;

        // Instantiate component
        let instance = pre
            .instantiate_async(&mut store)
            .await
            .context("failed to instantiate plugin")?;
        // Call the plugin host bindings to get metadata. If this succeeds, we know that
        // the component is a valid wash plugin.
        let plugin = bindings::WashPlugin::new(&mut store, &instance)
            .context("failed to create plugin host bindings")?;
        let metadata = plugin.wasmcloud_wash_plugin().call_info(&mut store).await?;

        // Determine the filesystem root based on the plugin name
        let wasi_fs_root = data_dir.map(|dir| {
            dir.as_ref()
                .join(PLUGINS_DIR)
                .join(PLUGINS_FS_DIR)
                .join(sanitize_plugin_name(&metadata.name))
        });

        if let Some(ref fs_root) = wasi_fs_root {
            // Ensure the filesystem root directory exists
            tokio::fs::create_dir_all(fs_root)
                .await
                .context("failed to create plugin filesystem root directory")?;
        }

        Ok(Self {
            id: component_id.to_owned(),
            workload: workload.to_owned(),
            metadata,
            wasi_fs_root,
            skip_confirmation,
        })
    }

    /// Call the plugin's `call_info` method to retrieve its metadata. Only use this
    /// method if you need to get metadata without instantiating the component. Otherwise,
    /// [`PluginComponent::metadata`] already contains the metadata.
    pub async fn call_info(&self) -> anyhow::Result<Metadata> {
        // Create a new store with the default context
        let mut store = self.workload.new_store(&self.id).await?;
        let component = self.workload.instantiate_pre(&self.id).await?;
        // Instantiate the component and call the plugin host bindings to get metadata
        let instance = component
            .instantiate_async(&mut store)
            .await
            .context("failed to instantiate plugin")?;
        // Get bindings
        let plugin = bindings::WashPlugin::new(&mut store, &instance)
            .context("failed to create plugin host bindings")?;
        plugin.wasmcloud_wash_plugin().call_info(&mut store).await
    }

    /// Instantiate a new instance of this component and call a specific hook
    /// with the provided context
    pub async fn call_hook(
        &self,
        hook: HookType,
        runner_context: Arc<RwLock<HashMap<String, String>>>,
    ) -> anyhow::Result<String> {
        let mut store = self.workload.new_store(&self.id).await?;
        let component = self.workload.instantiate_pre(&self.id).await?;
        let instance = component
            .instantiate_async(&mut store)
            .await
            .context("failed to instantiate plugin")?;
        let plugin = WashPlugin::new(&mut store, &instance)
            .context("failed to create plugin host bindings")?;

        // We'll use the same runner for initialization and the hook
        let runner = Runner::new(
            self.metadata.clone(),
            runner_context,
            self.skip_confirmation,
        );

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
        command: &bindings::wasmcloud::wash::types::Command,
        runner_context: Arc<RwLock<HashMap<String, String>>>,
    ) -> anyhow::Result<String> {
        let mut store = self.workload.new_store(&self.id).await?;
        let component = self.workload.instantiate_pre(&self.id).await?;
        let instance = component
            .instantiate_async(&mut store)
            .await
            .context("failed to instantiate plugin")?;
        let plugin = WashPlugin::new(&mut store, &instance)
            .context("failed to create plugin host bindings")?;
        let runner = Runner::new(
            self.metadata.clone(),
            runner_context,
            self.skip_confirmation,
        );

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

    /// Retrieve the original component [`Bytes`] of an installed plugin
    pub async fn get_original_component(&self, ctx: &CliContext) -> anyhow::Result<Bytes> {
        let path = self.path(ctx);
        Ok(tokio::fs::read(path).await.map(Bytes::from_owner)?)
    }

    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub fn path(&self, ctx: &CliContext) -> PathBuf {
        let sanitized_name = sanitize_plugin_name(&self.metadata.name);
        ctx.data_dir()
            .join(PLUGINS_DIR)
            .join(format!("{sanitized_name}.wasm"))
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
            pull_component(&options.source, oci_config)
                .await
                .with_context(|| {
                    format!(
                        "failed to pull plugin from OCI registry: {}",
                        options.source
                    )
                })?
                .0 // pull_component returns (bytes, digest), we only need bytes
        };

    let plugin = ctx.instantiate_plugin(component_data.clone()).await?;
    let metadata = plugin.metadata();

    // Validate that plugin commands don't conflict with built-in commands
    validate_plugin_commands(metadata)?;

    // Validate that plugin commands don't conflict with existing plugins
    validate_plugin_conflicts(ctx, metadata).await?;

    // Sanitize plugin name for filesystem storage
    let plugin_path = plugin.path(ctx);

    // Check if plugin already exists
    if plugin_path.exists() {
        if options.force {
            // uninstall first, to really force clean re-install
            uninstall_plugin(ctx, &metadata.name).await?;
        } else {
            bail!(
                "Plugin '{}' already exists. Use --force option to overwrite",
                metadata.name
            );
        }
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
        name: metadata.name.to_string(),
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
pub async fn list_plugins(ctx: &CliContext) -> anyhow::Result<Vec<Arc<PluginComponent>>> {
    // Sort plugins by name
    let mut plugins = ctx.plugin_manager().get_plugins().await;
    plugins.sort_by(|a, b| a.metadata.name.cmp(&b.metadata.name));

    Ok(plugins)
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
    let existing_plugins = ctx.plugin_manager().get_plugins().await;

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
    use crate::plugin::bindings::wasmcloud::wash::types::{Command, HookType};

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
