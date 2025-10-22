//! The main module for the wash CLI, providing command line interface functionality

use std::{
    collections::HashMap,
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context as _, bail, ensure};
use bytes::Bytes;
use etcetera::{
    AppStrategy, AppStrategyArgs,
    app_strategy::{Windows, Xdg},
    choose_app_strategy,
};
use tokio::sync::RwLock;

use serde_json::json;
use tracing::{debug, error, info, instrument, trace};

use serde::{Deserialize, Serialize};
use wash_runtime::{
    host::{Host, HostApi as _},
    plugin::wasi_config::RuntimeConfig,
    types::{Component, Workload, WorkloadStartRequest, WorkloadState},
    wit::WitInterface,
};

use crate::{
    CARGO_PKG_VERSION,
    cli::update::fetch_latest_release_public,
    config::{Config, generate_default_config, load_config},
    plugin::{PluginComponent, PluginManager, bindings::wasmcloud::wash::types::HookType},
};

pub mod completion;
pub mod component_build;
pub mod config;
/// Developer hot-reload loop for Wasm components
pub mod dev;
pub mod doctor;
pub mod inspect;
pub mod new;
pub mod oci;
pub mod plugin;
pub mod update;

pub const CONFIG_FILE_NAME: &str = "config.json";

/// A trait that defines the interface for all CLI commands
pub trait CliCommand {
    /// Execute the command with the provided context, returning a structured output
    fn handle(&self, ctx: &CliContext) -> impl Future<Output = anyhow::Result<CommandOutput>>;

    /// Enable pre-hook execution for this command
    fn enable_pre_hook(&self) -> Option<HookType> {
        None
    }

    /// Enable post-hook execution for this command
    fn enable_post_hook(&self) -> Option<HookType> {
        None
    }
}

impl<T: CliCommand + ?Sized> CliCommandExt for T {}

/// Extension trait to provide implementations for pre_hook and post_hook.
pub trait CliCommandExt: CliCommand {
    /// Execute pre-hook logic before the command runs. By default, if [`CliCommand::enable_pre_hook`]
    /// returns a hook type, it will execute all components registered with the pre-hook for that type.
    fn pre_hook(&self, ctx: &CliContext) -> impl Future<Output = anyhow::Result<()>> {
        async {
            if let Some(hook_type) = self.enable_pre_hook() {
                trace!(?hook_type, "executing pre-hooks for command");

                // TODO: propagate runtime context for modifying CLI behavior
                ctx.call_hooks(hook_type, Arc::default()).await
            }
            Ok(())
        }
    }

    /// Execute post-hook logic after the command runs. By default, if [`CliCommand::enable_post_hook`]
    /// returns a hook type, it will execute all components registered with the post-hook for that type.
    fn post_hook(&self, ctx: &CliContext) -> impl Future<Output = anyhow::Result<()>> {
        async {
            if let Some(hook_type) = self.enable_post_hook() {
                trace!(?hook_type, "executing post-hooks for command");
                // TODO: propagate runtime context for modifying CLI behavior
                ctx.call_hooks(hook_type, Arc::default()).await
            }
            Ok(())
        }
    }
}

/// Used for displaying human-readable output vs JSON format
#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum OutputKind {
    Text,
    Json,
}

impl std::str::FromStr for OutputKind {
    type Err = OutputParseErr;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "json" => Ok(Self::Json),
            "plain" | "text" => Ok(Self::Text),
            _ => Err(OutputParseErr),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputParseErr;

impl std::error::Error for OutputParseErr {}

impl std::fmt::Display for OutputParseErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "error parsing output type, see help for the list of accepted outputs"
        )
    }
}

/// The final output for a wash CLI command
#[derive(Debug, Clone, Serialize)]
pub struct CommandOutput {
    /// The message to display to the user
    message: String,
    /// Whether or not the command was successful
    success: bool,
    /// Additional data that can be included in JSON output
    data: Option<serde_json::Value>,
    /// The kind of output requested (text or JSON)
    #[serde(skip_serializing)]
    output_kind: OutputKind,
}

impl CommandOutput {
    pub fn ok(message: impl ToString, data: Option<serde_json::Value>) -> Self {
        Self {
            message: message.to_string(),
            success: true,
            data,
            output_kind: OutputKind::Text, // Default to Text, can be overridden later
        }
    }

    pub fn error(message: impl ToString, data: Option<serde_json::Value>) -> Self {
        Self {
            message: message.to_string(),
            success: false,
            data,
            output_kind: OutputKind::Text, // Default to Text, can be overridden later
        }
    }

    pub fn with_output_kind(self, output_kind: OutputKind) -> Self {
        Self {
            output_kind,
            ..self
        }
    }

    /// Check if the command was successful
    pub fn is_success(&self) -> bool {
        self.success
    }

    /// Get the text message from the output
    pub fn text(&self) -> &str {
        &self.message
    }

    /// Get the JSON data from the output
    pub fn json(&self) -> Option<&serde_json::Value> {
        self.data.as_ref()
    }

    /// Render the output as a string, returning the CLI message and whether it was successful
    pub fn render(self) -> (String, bool) {
        (
            match self.output_kind {
                OutputKind::Json => serde_json::to_string_pretty(&self).unwrap_or_else(|e| {
                    // Note that this matches the same structure as the CommandOutput
                    json!({
                        "message": "failed to serialize output",
                        "success": false,
                        "data": {
                            "error": e.to_string(),
                        }
                    })
                    .to_string()
                }),
                OutputKind::Text => self.message,
            },
            self.success,
        )
    }
}

/// Non-generic copy of etcetera::AppStrategy trait to make it dyn-compatible
pub trait DirectoryStrategy: std::fmt::Debug + Send + Sync {
    fn home_dir(&self) -> &Path;
    fn config_dir(&self) -> PathBuf;
    fn data_dir(&self) -> PathBuf;
    fn cache_dir(&self) -> PathBuf;
    fn state_dir(&self) -> Option<PathBuf>;
    fn runtime_dir(&self) -> Option<PathBuf>;
    fn in_config_dir(&self, path: &str) -> PathBuf {
        let mut p = self.config_dir();
        p.push(Path::new(&path));
        p
    }
    fn in_data_dir(&self, path: &str) -> PathBuf {
        let mut p = self.data_dir();
        p.push(Path::new(&path));
        p
    }
    fn in_cache_dir(&self, path: &str) -> PathBuf {
        let mut p = self.cache_dir();
        p.push(Path::new(&path));
        p
    }
    fn in_state_dir(&self, path: &str) -> Option<PathBuf> {
        let mut p = self.state_dir()?;
        p.push(Path::new(&path));
        Some(p)
    }
    fn in_runtime_dir(&self, path: &str) -> Option<PathBuf> {
        let mut p = self.runtime_dir()?;
        p.push(Path::new(&path));
        Some(p)
    }
}

impl DirectoryStrategy for Xdg {
    fn home_dir(&self) -> &Path {
        <Xdg as AppStrategy>::home_dir(self)
    }
    fn config_dir(&self) -> PathBuf {
        <Xdg as AppStrategy>::config_dir(self)
    }
    fn data_dir(&self) -> PathBuf {
        <Xdg as AppStrategy>::data_dir(self)
    }
    fn cache_dir(&self) -> PathBuf {
        <Xdg as AppStrategy>::cache_dir(self)
    }
    fn state_dir(&self) -> Option<PathBuf> {
        <Xdg as AppStrategy>::state_dir(self)
    }
    fn runtime_dir(&self) -> Option<PathBuf> {
        <Xdg as AppStrategy>::runtime_dir(self)
    }
}

impl DirectoryStrategy for Windows {
    fn home_dir(&self) -> &Path {
        <Windows as AppStrategy>::home_dir(self)
    }
    fn config_dir(&self) -> PathBuf {
        <Windows as AppStrategy>::config_dir(self)
    }
    fn data_dir(&self) -> PathBuf {
        <Windows as AppStrategy>::data_dir(self)
    }
    fn cache_dir(&self) -> PathBuf {
        <Windows as AppStrategy>::cache_dir(self)
    }
    fn state_dir(&self) -> Option<PathBuf> {
        <Windows as AppStrategy>::state_dir(self)
    }
    fn runtime_dir(&self) -> Option<PathBuf> {
        <Windows as AppStrategy>::runtime_dir(self)
    }
}

/// CliContext holds the global context for the wash CLI, including output kind and directories
///
/// It is used to manage configuration, data, and cache directories based on the XDG Base Directory Specification,
/// or a custom configuration if needed.
#[derive(Debug, Clone)]
pub struct CliContext {
    /// Application strategy to access configuration directories.
    app_strategy: Arc<dyn DirectoryStrategy>,
    /// A wasmCloud host instance used for executing plugins
    host: Arc<Host>,
    plugin_manager: Arc<PluginManager>,
}

impl Deref for CliContext {
    type Target = Arc<dyn DirectoryStrategy>;

    fn deref(&self) -> &Arc<dyn DirectoryStrategy> {
        &self.app_strategy
    }
}

/// Builder for constructing a CliContext
#[derive(Default)]
pub struct CliContextBuilder {
    non_interactive: bool,
}

impl CliContextBuilder {
    pub fn non_interactive(mut self, non_interactive: bool) -> Self {
        self.non_interactive = non_interactive;
        self
    }

    /// Build the CliContext
    pub async fn build(self) -> anyhow::Result<CliContext> {
        let app_strategy: Arc<dyn DirectoryStrategy> = Arc::new(
            choose_app_strategy(AppStrategyArgs {
                top_level_domain: "com.wasmcloud".to_string(),
                author: "wasmCloud Team".to_string(),
                app_name: "wash".to_string(),
            })
            .context("failed to to determine file system strategy")?,
        );

        if app_strategy.data_dir().exists() {
            trace!(
                dir = ?app_strategy.data_dir(),
                "data directory already exists, skipping creation"
            );
        } else {
            debug!(
                dir = ?app_strategy.data_dir(),
                "creating data directory for wash CLI"
            );
            tokio::fs::create_dir_all(app_strategy.data_dir())
                .await
                .context("failed to create data directory")?;
        }

        if app_strategy.cache_dir().exists() {
            trace!(
                dir = ?app_strategy.cache_dir(),
                "cache directory already exists, skipping creation"
            );
        } else {
            debug!(
                dir = ?app_strategy.cache_dir(),
                "creating cache directory for wash CLI"
            );
            tokio::fs::create_dir_all(app_strategy.cache_dir())
                .await
                .context("failed to create cache directory")?;
        }

        if app_strategy.config_dir().exists() {
            trace!(
                dir = ?app_strategy.config_dir(),
                "config directory already exists, skipping creation"
            );
        } else {
            debug!(
                dir = ?app_strategy.config_dir(),
                "creating config directory for wash CLI"
            );
            tokio::fs::create_dir_all(app_strategy.config_dir())
                .await
                .context("failed to create config directory")?;
        }

        let plugin_manager = Arc::new(PluginManager::new(self.non_interactive));

        let host = wash_runtime::host::Host::builder()
            .with_plugin(Arc::new(RuntimeConfig::default()))?
            .with_plugin(plugin_manager.clone())?
            .build()
            .context("failed to create wash runtime")?
            .start()
            .await?;

        let ctx = CliContext {
            app_strategy,
            host,
            plugin_manager: plugin_manager.clone(),
        };

        // Once the CliContext is initialized, load all plugins
        plugin_manager.load_plugins(&ctx, ctx.data_dir()).await?;

        Ok(ctx)
    }
}

impl CliContext {
    /// Creates a new CliContext builder with default settings
    pub fn builder() -> CliContextBuilder {
        CliContextBuilder::default()
    }

    /// Returns whether wash is running in non-interactive mode
    pub fn is_non_interactive(&self) -> bool {
        self.plugin_manager().skip_confirmation()
    }

    #[instrument(level = "debug", skip(self))]
    pub async fn check_new_version(&self) -> anyhow::Result<bool> {
        debug!("checking for new version of wash");

        let new_version_available = self.app_strategy.in_cache_dir("new_version_available.txt");
        if new_version_available.exists() {
            trace!(
                ?new_version_available,
                "found new_version_available.txt in cache directory"
            );
            let contents = tokio::fs::read_to_string(&new_version_available)
                .await
                .context("failed to read new_version_available.txt")?;
            if let (Ok(new_ver), Ok(cur_ver)) = (
                semver::Version::parse(contents.trim()),
                semver::Version::parse(CARGO_PKG_VERSION),
            ) && new_ver > cur_ver
            {
                return Ok(true);
            }
        }

        if let Ok(release) = fetch_latest_release_public().await {
            trace!(?release, "fetched latest release from GitHub");
            let tagged_version = release
                .tag_name
                .strip_prefix("wash-v")
                .unwrap_or(&release.tag_name);

            debug!(ver = ?tagged_version, "determined tagged version");
            if let Ok(new_ver) = semver::Version::parse(tagged_version)
                && let Ok(cur_ver) = semver::Version::parse(CARGO_PKG_VERSION)
            {
                debug!(cur_ver = ?cur_ver, new_ver = ?new_ver, "comparing versions");
                if new_ver > cur_ver {
                    // Write the new version to the cache file
                    tokio::fs::write(&new_version_available, tagged_version)
                        .await
                        .context("failed to write new version to cache file")?;
                    return Ok(true);
                }
            }
        } else {
            debug!("failed to check for latest release, continuing without update check");
        }

        Ok(false)
    }

    pub fn config_path(&self) -> std::path::PathBuf {
        self.app_strategy.in_config_dir(CONFIG_FILE_NAME)
    }

    /// Fetches the wash configuration from the config file located in the XDG config directory,
    /// creating it with default values if it does not exist.
    pub async fn ensure_config(&self, project_dir: Option<&Path>) -> anyhow::Result<Config> {
        let config_path = self.config_path();

        // Check if the config file exists, if not create it with defaults
        if !config_path.exists() {
            debug!(
                ?config_path,
                "config file not found, creating with defaults"
            );
            generate_default_config(&config_path, false, true).await?;
        }

        // Load the configuration using the hierarchical configuration system
        load_config(&self.config_path(), project_dir, None::<Config>)
    }

    /// Convenience method to quickly instantiate a [`PluginComponent`] from bytes, generally
    /// for ensuring that the bytes are a valid component and for fetching metadata. This function
    /// does not install the plugin on disk and it will be lost when the [`CliContext`] is dropped.
    pub async fn instantiate_plugin(
        &self,
        plugin_bytes: impl Into<Bytes>,
    ) -> anyhow::Result<Arc<PluginComponent>> {
        // Validate that it's a valid WebAssembly component and wash plugin
        let workload = Workload {
            namespace: "default".to_string(),
            name: "temp-plugin".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                bytes: plugin_bytes.into(),
                ..Default::default()
            }],
            host_interfaces: vec![
                WitInterface::from("wasmcloud:wash/types@0.0.2"),
                WitInterface::from("wasi:config/runtime@0.2.0-draft"),
            ],
            // TODO: Messes with host interface parsing
            // host_interfaces: vec![WitInterface::from("wasmcloud:wash/plugin,types@0.0.2")],
            volumes: vec![],
        };

        let res = self
            .host()
            .workload_start(WorkloadStartRequest { workload })
            .await?;
        ensure!(
            res.workload_status.workload_state == WorkloadState::Running,
            "plugin failed to instantiate during install"
        );

        let Some(plugin) = self
            .plugin_manager()
            .get_plugin_by_workload_id(res.workload_status.workload_id)
            .await
        else {
            bail!("plugin failed to install in CLI context")
        };

        Ok(plugin)
    }

    pub fn plugin_manager(&self) -> &PluginManager {
        &self.plugin_manager
    }

    pub fn host(&self) -> &Arc<Host> {
        &self.host
    }

    /// Call hooks for the specified hook type with the provided runtime context.
    /// This will execute ALL plugins that support the given hook type.
    pub async fn call_hooks(
        &self,
        hook_type: HookType,
        runtime_context: Arc<RwLock<HashMap<String, String>>>,
    ) {
        let hooks = self.plugin_manager.get_hooks(hook_type).await;
        for hook in hooks {
            trace!(?hook, ?hook_type, "executing hook");

            // Hook errors do not cause the CLI to stop execution, we just log either way
            match hook.call_hook(hook_type, runtime_context.clone()).await {
                Ok(response) => {
                    info!(
                        plugin = hook.metadata.name,
                        ?hook_type,
                        response,
                        "hook executed successfully"
                    );
                }
                Err(e) => {
                    error!(
                        err = ?e,
                        ?hook_type,
                        plugin = hook.metadata.name,
                        "hook execution failed"
                    );
                }
            }
        }
    }
}
