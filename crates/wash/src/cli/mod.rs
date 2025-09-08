//! The main module for the wash CLI, providing command line interface functionality

use std::{ops::Deref, path::Path, sync::Arc};

use anyhow::Context as _;
use etcetera::{AppStrategy as _, AppStrategyArgs, choose_app_strategy};
use tokio::{process::Child, sync::RwLock};
use tracing::info;

#[cfg(windows)]
use etcetera::app_strategy::Windows;
#[cfg(unix)]
use etcetera::app_strategy::Xdg;

use serde_json::json;
use tracing::{debug, error, instrument, trace};

use serde::{Deserialize, Serialize};

use crate::{
    CARGO_PKG_VERSION,
    cli::update::fetch_latest_release_public,
    config::{Config, generate_default_config, load_config},
    plugin::PluginManager,
    runtime::{
        Ctx,
        bindings::plugin::{WashPlugin, exports::wasmcloud::wash::plugin::HookType},
        new_runtime,
        plugin::Runner,
    },
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
                let hooks = ctx.plugin_manager.get_hooks(hook_type);
                for hook in hooks {
                    trace!(?hook, ?hook_type, "executing pre-hook for command");
                    let mut data = Ctx::builder(hook.metadata.name.clone())
                        .with_background_processes(ctx.background_processes.clone())
                        .build();
                    // TODO(IMPORTANT): context about the command and runner
                    let runner = data
                        .table
                        .push(Runner::new(hook.metadata.clone(), Arc::default()))?;
                    let mut store = hook.component.new_store(data);
                    let instance = hook
                        .component
                        .instance_pre()
                        .instantiate_async(&mut store)
                        .await
                        .context("failed to instantiate pre-hook")?;
                    let plugin_guest = WashPlugin::new(&mut store, &instance)?;
                    if let Err(e) = plugin_guest
                        .wasmcloud_wash_plugin()
                        .call_hook(&mut store, runner, hook_type)
                        .await
                        .context("failed to call pre-hook")?
                    {
                        error!(
                            err = e,
                            name = hook.metadata.name,
                            "pre-hook execution failed"
                        );
                    }
                }
            }
            Ok(())
        }
    }

    /// Execute post-hook logic after the command runs. By default, if [`CliCommand::enable_post_hook`]
    /// returns a hook type, it will execute all components registered with the post-hook for that type.
    fn post_hook(&self, ctx: &CliContext) -> impl Future<Output = anyhow::Result<()>> {
        async {
            if let Some(hook_type) = self.enable_post_hook() {
                let hooks = ctx.plugin_manager.get_hooks(hook_type);
                for hook in hooks {
                    trace!(?hook, "executing post-hook for command");
                    let mut data = Ctx::builder(hook.metadata.name.clone())
                        .with_background_processes(ctx.background_processes.clone())
                        .build();
                    // TODO(IMPORTANT): context about the command and runner
                    let runner = data
                        .table
                        .push(Runner::new(hook.metadata.clone(), Arc::default()))?;
                    let mut store = hook.component.new_store(data);
                    let instance = hook
                        .component
                        .instance_pre()
                        .instantiate_async(&mut store)
                        .await
                        .context("failed to instantiate post-hook")?;
                    let plugin_guest = WashPlugin::new(&mut store, &instance)?;
                    if let Err(e) = plugin_guest
                        .wasmcloud_wash_plugin()
                        .call_hook(&mut store, runner, hook_type)
                        .await
                        .context("failed to call post-hook")?
                    {
                        error!(
                            err = e,
                            name = hook.metadata.name,
                            "post-hook execution failed"
                        );
                    }
                }
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

/// CliContext holds the global context for the wash CLI, including output kind and directories
///
/// It is used to manage configuration, data, and cache directories based on the XDG Base Directory Specification,
/// or a custom configuration if needed.
#[derive(Debug, Clone)]
pub struct CliContext {
    // TODO(#25): Just store an Arc-ed trait object
    #[cfg(unix)]
    app_strategy: Xdg,
    #[cfg(windows)]
    app_strategy: Windows,
    /// The runtime used for executing Wasm components. Plugins and
    /// dev loops will use this runtime to execute Wasm code.
    runtime: wasmcloud_runtime::Runtime,
    runtime_thread: Arc<std::thread::JoinHandle<Result<(), ()>>>,
    plugin_manager: Arc<PluginManager>,
    /// Stores the handles to background processes spawned by host_exec_background. We want to
    /// constrain the processes spawned by components to the lifetime of the CLI context.
    background_processes: Arc<RwLock<Vec<Child>>>,
}

#[cfg(unix)]
impl Deref for CliContext {
    type Target = Xdg;

    fn deref(&self) -> &Xdg {
        &self.app_strategy
    }
}
#[cfg(windows)]
impl Deref for CliContext {
    type Target = Windows;

    fn deref(&self) -> &Windows {
        &self.app_strategy
    }
}

impl CliContext {
    /// Creates a new [CliContext] with the specified output kind and directory paths.
    pub async fn new() -> anyhow::Result<Self> {
        let app_strategy = choose_app_strategy(AppStrategyArgs {
            top_level_domain: "com.wasmcloud".to_string(),
            author: "wasmCloud Team".to_string(),
            app_name: "wash".to_string(),
        })
        .context("failed to to determine file system strategy")?;

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

        let (plugin_runtime, thread) = new_runtime()
            .await
            .context("failed to create wasmcloud runtime")?;

        let plugin_manager = PluginManager::initialize(&plugin_runtime, app_strategy.data_dir())
            .await
            .context("failed to initialize plugin manager")?;

        Ok(Self {
            app_strategy,
            runtime: plugin_runtime,
            runtime_thread: Arc::new(thread),
            plugin_manager: Arc::new(plugin_manager),
            background_processes: Arc::default(),
        })
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

    pub fn runtime(&self) -> &wasmcloud_runtime::Runtime {
        &self.runtime
    }
    pub fn runtime_thread(&self) -> &Arc<std::thread::JoinHandle<Result<(), ()>>> {
        &self.runtime_thread
    }
    pub fn plugin_manager(&self) -> &PluginManager {
        &self.plugin_manager
    }

    /// Call pre-hooks for the specified hook type with the provided runtime context.
    /// This will execute ALL plugins that support the given hook type.
    pub async fn call_pre_hooks(
        &self,
        runtime_context: std::sync::Arc<
            tokio::sync::RwLock<std::collections::HashMap<String, String>>,
        >,
        hook_type: HookType,
    ) -> anyhow::Result<()> {
        let hooks = self.plugin_manager.get_hooks(hook_type);
        for hook in hooks {
            trace!(?hook, ?hook_type, "executing pre-hook");
            let mut data = Ctx::builder(hook.metadata.name.clone())
                .with_background_processes(self.background_processes.clone())
                .build();
            let runner = data
                .table
                .push(Runner::new(hook.metadata.clone(), runtime_context.clone()))?;
            let mut store = hook.component.new_store(data);
            let instance = hook
                .component
                .instance_pre()
                .instantiate_async(&mut store)
                .await
                .context("failed to instantiate pre-hook")?;
            let plugin_guest = WashPlugin::new(&mut store, &instance)?;
            match plugin_guest
                .wasmcloud_wash_plugin()
                .call_hook(&mut store, runner, hook_type)
                .await
                .context("failed to call pre-hook")?
            {
                Ok(response) => {
                    info!(
                        plugin = hook.metadata.name,
                        response = response,
                        "pre-hook executed successfully"
                    );
                }
                Err(e) => {
                    error!(
                        err = e,
                        plugin = hook.metadata.name,
                        "pre-hook execution failed"
                    );
                }
            }
        }
        Ok(())
    }

    /// Call post-hooks for the specified hook type with the provided runtime context.
    /// This will execute ALL plugins that support the given hook type.
    pub async fn call_post_hooks(
        &self,
        runtime_context: std::sync::Arc<
            tokio::sync::RwLock<std::collections::HashMap<String, String>>,
        >,
        hook_type: HookType,
    ) -> anyhow::Result<()> {
        let hooks = self.plugin_manager.get_hooks(hook_type);
        for hook in hooks {
            trace!(?hook, ?hook_type, "executing post-hook");
            let mut data = Ctx::builder(hook.metadata.name.clone())
                .with_background_processes(self.background_processes.clone())
                .build();
            let runner = data
                .table
                .push(Runner::new(hook.metadata.clone(), runtime_context.clone()))?;
            let mut store = hook.component.new_store(data);
            let instance = hook
                .component
                .instance_pre()
                .instantiate_async(&mut store)
                .await
                .context("failed to instantiate post-hook")?;
            let plugin_guest = WashPlugin::new(&mut store, &instance)?;
            match plugin_guest
                .wasmcloud_wash_plugin()
                .call_hook(&mut store, runner, hook_type)
                .await
                .context("failed to call post-hook")?
            {
                Ok(response) => {
                    info!(
                        plugin = hook.metadata.name,
                        response = response,
                        "post-hook executed successfully"
                    );
                }
                Err(e) => {
                    error!(
                        err = e,
                        plugin = hook.metadata.name,
                        "post-hook execution failed"
                    );
                }
            }
        }
        Ok(())
    }
}
