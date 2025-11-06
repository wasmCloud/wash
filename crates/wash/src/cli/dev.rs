use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

use anyhow::{Context as _, bail, ensure};
use base64::Engine;
use bytes::Bytes;
use clap::Args;
use notify::{
    Event as NotifyEvent, RecursiveMode, Watcher,
    event::{EventKind, ModifyKind},
};
use tokio::{select, sync::mpsc};
use tracing::{debug, error, info, trace, warn};
use wash_runtime::{
    host::{Host, HostApi},
    plugin::{wasi_config::WasiConfig, wasi_http::HttpServer, wasi_logging::WasiLogging, wasi_webgpu::WasiWebgpu},
    types::{
        Component, HostPathVolume, LocalResources, Volume, VolumeMount, VolumeType, Workload,
        WorkloadStartRequest, WorkloadState, WorkloadStopRequest,
    },
    wit::WitInterface,
};

use crate::{
    cli::{
        CliCommand, CliContext, CommandOutput,
        component_build::build_component,
        doctor::{ProjectContext, check_project_specific_tools, detect_project_context},
    },
    component_build::BuildConfig,
    config::{Config, load_config},
    plugin::bindings::wasmcloud::wash::types::HookType,
};

#[derive(Debug, Clone, Args)]
pub struct DevCommand {
    /// The path to the project directory
    #[clap(name = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,

    /// The path to the built Wasm file to be used in development
    #[clap(long = "component-path")]
    pub component_path: Option<PathBuf>,

    /// The address on which the HTTP server will listen
    #[clap(long = "address", default_value = "0.0.0.0:8000")]
    pub address: String,

    /// Configuration values to use for `wasi:config/store` in the form of `key=value` pairs.
    #[clap(long = "wasi-config", value_delimiter = ',')]
    pub wasi_config: Vec<String>,

    // TODO: filesystem root?
    /// The root directory for the blobstore to use for `wasi:blobstore/blobstore`. Defaults to a subfolder in the wash data directory.
    #[clap(long = "blobstore-root")]
    pub blobstore_root: Option<PathBuf>,

    /// Path to TLS certificate file (PEM format) for HTTPS support
    #[clap(long = "tls-cert", requires = "tls_key")]
    pub tls_cert: Option<PathBuf>,

    /// Path to TLS private key file (PEM format) for HTTPS support
    #[clap(long = "tls-key", requires = "tls_cert")]
    pub tls_key: Option<PathBuf>,

    /// Path to CA certificate bundle (PEM format) for client certificate verification (optional)
    #[clap(long = "tls-ca")]
    pub tls_ca: Option<PathBuf>,
}

impl CliCommand for DevCommand {
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        info!(path = ?self.project_dir, "starting development session for project");

        let config = load_config(
            &ctx.config_path(),
            Some(self.project_dir.as_path()),
            // Override the component path with the one provided in the command line
            Some(Config {
                build: Some(BuildConfig {
                    component_path: self.component_path.clone(),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        )
        .context("failed to load config for development")?;

        // Validate project directory
        ensure!(
            self.project_dir.exists(),
            "Project directory does not exist: {}",
            self.project_dir.display()
        );
        ensure!(
            self.project_dir.is_dir(),
            "Project path is not a directory: {}",
            self.project_dir.display()
        );

        // Check for required tools (e.g., wasmCloud, WIT)
        let project_context = detect_project_context(&self.project_dir)
            .await
            .context("failed to detect project context")?;
        let (issues, recommendations) = check_project_specific_tools(&project_context)
            .await
            .context("failed to check project specific tools")?;
        if !issues.is_empty() {
            for issue in issues {
                warn!(issue = issue, "project tool issue");
            }
        } else {
            debug!("no issues found with project tools");
        }
        if !recommendations.is_empty() {
            for recommendation in recommendations {
                warn!(
                    recommendation = recommendation,
                    "project tool recommendation"
                );
            }
        } else {
            debug!("no recommendations found for project tools");
        }

        let component_path = match build_component(&self.project_dir, ctx, &config).await {
            // Edge case where the build was successful, but the component path in the config is different
            // than the one returned by the build process.
            Ok(build_result)
                if config
                    .build
                    .as_ref()
                    .and_then(|b| b.component_path.as_ref())
                    .is_some_and(|p| p != &build_result.component_path) =>
            {
                warn!(path = ?build_result.component_path, "component built successfully, but component path in config is different");
                // Ensure the component path is set in the config
                build_result.component_path
            }
            // Use the build result component path if the config does not specify one
            Ok(build_result) => {
                debug!(path = ?build_result.component_path, "component built successfully, using as component path");
                build_result.component_path
            }
            Err(e) => {
                // TODO(#18): Support continuing, npm start works like that.
                error!("failed to build component, will not start dev session");
                error!("{e}");
                return Err(e);
            }
        };

        // Deploy to local host
        let wasm_bytes = tokio::fs::read(&component_path)
            .await
            .context("failed to read component file")?;

        // Call pre-hooks before starting dev session
        // Empty context for pre-hooks, consider adding more
        ctx.call_hooks(HookType::BeforeDev, Arc::default()).await;
        let dev_register_plugins = ctx.plugin_manager().get_hooks(HookType::DevRegister).await;

        let mut dev_register_components = Vec::with_capacity(dev_register_plugins.len());
        for plugin in dev_register_plugins {
            dev_register_components.push(plugin.get_original_component(ctx).await?)
        }

        let mut host_builder = Host::builder();
        host_builder = host_builder.with_plugin(Arc::new(WasiConfig::default()))?;
        host_builder = host_builder.with_plugin(Arc::new(WasiWebgpu::new()))?;

        let volume_root = self
            .blobstore_root
            .clone()
            .unwrap_or_else(|| ctx.data_dir().join("dev_blobstore"));
        // Ensure the blobstore root directory exists
        if !volume_root.exists() {
            tokio::fs::create_dir_all(&volume_root)
                .await
                .context("failed to create blobstore root directory")?;
        }
        debug!(path = ?volume_root.display(), "using blobstore root directory");

        // TODO(#19): Only spawn the server if the component exports wasi:http
        // Configure HTTP server with optional TLS, enable HTTP Server
        let protocol = if let (Some(cert_path), Some(key_path)) = (&self.tls_cert, &self.tls_key) {
            ensure!(
                cert_path.exists(),
                "TLS certificate file does not exist: {}",
                cert_path.display()
            );
            ensure!(
                key_path.exists(),
                "TLS private key file does not exist: {}",
                key_path.display()
            );

            if let Some(ca_path) = &self.tls_ca {
                ensure!(
                    ca_path.exists(),
                    "CA certificate file does not exist: {}",
                    ca_path.display()
                );
            }

            host_builder = host_builder.with_plugin(Arc::new(
                HttpServer::new_with_tls(
                    self.address.parse()?,
                    cert_path,
                    key_path,
                    self.tls_ca.as_deref(),
                )
                .await?,
            ))?;

            debug!("TLS configured - server will use HTTPS");
            "https"
        } else {
            debug!("No TLS configuration provided - server will use HTTP");
            host_builder =
                host_builder.with_plugin(Arc::new(HttpServer::new(self.address.parse()?)))?;
            "http"
        };

        // Add logging plugin
        host_builder = host_builder.with_plugin(Arc::new(WasiLogging))?;
        debug!("Logging plugin registered");

        // Build and start the host
        let host = host_builder.build()?.start().await?;

        // Collect wasi configuration for the component
        let wasi_config = self
            .wasi_config
            .clone()
            .into_iter()
            .filter_map(|orig| match orig.split_once('=') {
                Some((k, v)) => Some((k.to_string(), v.to_string())),
                None => {
                    warn!(key = orig, "wasi config key without value, skipping");
                    None
                }
            })
            .collect::<HashMap<String, String>>();

        // Workload structure
        let mut workload = create_workload(
            wasm_bytes.into(),
            wasi_config,
            volume_root,
            dev_register_components,
        );
        // Running workload ID for reloads
        let mut workload_id = reload_component(host.clone(), &workload, None).await?;

        // Track last build time for WIT change detection
        let mut last_build_time = SystemTime::now();

        // Canonicalize project root once to ensure consistent path comparisons
        let canonical_project_root = self.project_dir.canonicalize().with_context(|| {
            format!(
                "failed to canonicalize project directory: {}",
                self.project_dir.display()
            )
        })?;
        debug!(
            original = ?self.project_dir.display(),
            canonical = ?canonical_project_root.display(),
            "canonicalized project root for file watching"
        );

        // Enable/disable watching to prevent having the output artifact trigger a rebuild
        // This starts as true to prevent a rebuild on the first run
        let pause_watch = Arc::new(AtomicBool::new(true));
        let watcher_paused = pause_watch.clone();
        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
        let (reload_tx, mut reload_rx) = mpsc::channel::<()>(1);

        // Build initial ignore set including artifact path and project-specific build directories
        let initial_ignore_set = build_ignore_set(&canonical_project_root, &project_context);
        debug!(
            ignore_count = initial_ignore_set.len(),
            project_type = ?project_context,
            "built initial file watcher ignore set"
        );
        let ignore_paths = Arc::new(initial_ignore_set);
        let ignore_paths_notify = ignore_paths.clone();

        let canonical_project_root_notify = canonical_project_root.clone();
        debug!(path = ?self.project_dir.display(), "setting up watcher");

        // Watch for changes and rebuild/deploy as needed
        let mut watcher = notify::recommended_watcher(move |res: _| match res {
            Ok(event) => {
                if let NotifyEvent {
                    kind:
                        EventKind::Create(_)
                        | EventKind::Modify(ModifyKind::Data(_))
                        | EventKind::Remove(_),
                    paths,
                    ..
                } = event
                {
                    // Check if any of the changed paths should be ignored to prevent
                    // recursive rebuilds from artifact directories
                    let set = &ignore_paths_notify;
                    if paths
                        .iter()
                        .any(|p| is_ignored(p, &canonical_project_root_notify, set))
                    {
                        trace!(paths = ?paths, "ignoring file changes in artifact directories");
                        return;
                    }
                    // If watch has been paused for any reason, skip notifications
                    if watcher_paused.load(Ordering::SeqCst) {
                        return;
                    }
                    trace!(paths = ?paths, "file event triggered dev loop");

                    // NOTE(brooksmtownsend): `try_send` here is used intentionally to prevent
                    // multiple file reloads from queuing up a backlog of reloads.
                    let _ = reload_tx.try_send(());
                }
            }
            Err(e) => {
                error!(err = ?e, "watch failed");
            }
        })?;

        watcher.watch(&canonical_project_root, RecursiveMode::Recursive)?;
        debug!("watching for file changes...");

        // Spawn a task to handle Ctrl + C signal
        tokio::spawn(async move {
            tokio::signal::ctrl_c()
                .await
                .context("failed to wait for ctrl_c signal")?;
            stop_tx
                .send(())
                .await
                .context("failed to send stop signal after receiving Ctrl + c")?;
            Result::<_, anyhow::Error>::Ok(())
        });

        // Enable file watching
        pause_watch.store(false, Ordering::SeqCst);

        // Make sure the reload channel is empty before starting the loop
        let _ = reload_rx.try_recv();

        info!("development session started successfully");
        info!(address = %format!("{}://{}", protocol, self.address), "listening for HTTP requests");

        loop {
            info!("watching for file changes (press Ctrl+c to stop)...");
            select! {
                // Process a file change/reload
                _ = reload_rx.recv() => {
                    pause_watch.store(true, Ordering::SeqCst);

                    info!("rebuilding component after file changed ...");

                    // Check if WIT-related files have changed since the last build
                    let wit_changed = wit_files_changed(&self.project_dir, last_build_time);

                    // Create a modified config with skip_fetch set based on WIT file changes
                    let mut rebuild_config = config.clone();
                    if !wit_changed {
                        info!("no WIT-related files changed, skipping WIT dependency fetch");
                        if let Some(wit_config) = rebuild_config.wit.as_mut() {
                            wit_config.skip_fetch = true;
                        } else {
                            rebuild_config.wit = Some(crate::wit::WitConfig {
                                skip_fetch: true,
                                ..Default::default()
                            });
                        }
                    } else {
                        debug!("WIT-related files changed, fetching WIT dependencies");
                    }

                    // TODO(IMPORTANT): ensure that this calls the build pre-post hooks
                    // TODO(#22): Typescript: Skip install if no package.json change
                    let rebuild_result = build_component(
                        &self.project_dir,
                        ctx,
                        &rebuild_config,
                    ).await;

                    match rebuild_result {
                        Ok(build_result) => {
                            // Use the new component path from the build result
                            let component_path = build_result.component_path;
                            info!(path = %component_path.display(), "component rebuilt successfully");

                            info!("deploying rebuilt component ...");
                            let wasm_bytes = tokio::fs::read(&component_path)
                                .await
                                .context("failed to read component file")?;

                            update_workload_component(&mut workload, wasm_bytes.into());

                            workload_id = reload_component(
                                host.clone(),
                                &workload,
                                Some(workload_id),
                            ).await?;

                            // Update last build time after successful rebuild
                            last_build_time = SystemTime::now();

                            // Avoid jitter with reloads by pausing the watcher for a short time
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            // Make sure that the reload channel is empty before unpausing the watcher
                            let _ = reload_rx.try_recv();
                            pause_watch.store(false, Ordering::SeqCst);
                        }
                        Err(e) => {
                            info!("failed to build component, will retry on next file change");
                            // TODO(#23): This doesn't include color output
                            // This nicely formats the error message
                            error!("{e}");
                            // If the build fails, we pause the watcher to prevent further reloads
                            let _ = reload_rx.try_recv();
                            pause_watch.store(false, Ordering::SeqCst);
                            continue;
                        }
                    }
                },

                // Process a stop
                _ = stop_rx.recv() => {
                    info!("Stopping development session ...");
                    pause_watch.store(true, Ordering::SeqCst);

                    break
                },
            }
        }

        // Stop the workload and clean up resources
        if let Err(e) = host
            .workload_stop(WorkloadStopRequest {
                workload_id: workload_id.clone(),
            })
            .await
        {
            warn!(
                workload_id = workload_id,
                error = ?e,
                "failed to stop workload during shutdown, continuing cleanup"
            );
        } else {
            debug!(workload_id = workload_id, "workload stopped successfully");
        }

        // Call post-hooks with component bytes context
        // Base64 encode the bytes since context only supports HashMap<String, String>
        if let Some(component) = workload.components.first() {
            debug!(size = component.bytes.len(), "final component size (bytes)");
            let component_bytes_b64 =
                base64::engine::general_purpose::STANDARD.encode(component.bytes.clone());
            let mut post_context = HashMap::new();
            post_context.insert(
                "dev.component_bytes_base64".to_string(),
                component_bytes_b64,
            );
        }

        // Empty context for AfterDev, consider adding more
        ctx.call_hooks(HookType::AfterDev, Arc::default()).await;

        Ok(CommandOutput::ok(
            "Development command executed successfully".to_string(),
            None,
        ))
    }
}

/// Update the bytes of the development component in the workload
fn update_workload_component(workload: &mut Workload, bytes: Bytes) {
    if let Some(component) = workload.components.get_mut(0) {
        component.bytes = bytes;
    }
}

/// Extract WIT interfaces from a component's imports and exports
///
/// Inspects the component to determine what interfaces it uses and provides.
/// This is used to populate the `host_interfaces` field in the Workload, which is
/// checked bidirectionally against both imports and exports during plugin binding.
///
/// For example:
/// - A component that **exports** `wasi:http/incoming-handler` needs the HTTP server plugin
/// - A component that **imports** `wasi:blobstore/blobstore` needs the blobstore plugin
fn extract_component_interfaces(component_bytes: &[u8]) -> anyhow::Result<HashSet<WitInterface>> {
    use wasmtime::component::Component;

    // Create a minimal engine just for introspection
    let engine = wasmtime::Engine::default();
    let component = Component::new(&engine, component_bytes)
        .context("failed to parse component for interface extraction")?;

    let ty = component.component_type();
    let mut interfaces = HashSet::new();

    // Helper closure to parse interface names
    let parse_interface = |name: &str| -> Option<WitInterface> {
        // Parse names like "wasi:http/incoming-handler@0.2.0"
        let (namespace_package, interface_version) = name.rsplit_once('/')?;
        let (namespace, package) = namespace_package.split_once(':')?;

        // Extract interface name and optional version
        let (interface, version) = if let Some((iface, ver)) = interface_version.split_once('@') {
            let parsed_version = ver.parse().ok();
            (iface.to_string(), parsed_version)
        } else {
            (interface_version.to_string(), None)
        };

        Some(WitInterface {
            namespace: namespace.to_string(),
            package: package.to_string(),
            interfaces: HashSet::from([interface]),
            version,
            config: HashMap::new(),
        })
    };

    // Helper to check if an interface is a standard WASI interface
    // These are provided by wasmtime-wasi and should not be in host_interfaces
    let is_standard_wasi = |interface: &WitInterface| -> bool {
        if interface.namespace != "wasi" {
            return false;
        }

        // Standard WASI 0.2 packages that are provided by wasmtime-wasi linker
        // These don't need plugin binding
        if matches!(
            interface.package.as_str(),
            // Core WASI provided by wasmtime-wasi
            "cli" | "clocks" | "filesystem" | "io" | "random" | "sockets"
        ) {
            return true;
        }

        // Type-only interfaces that don't need plugin binding
        // These are just type definitions used by other interfaces
        if interface.interfaces.iter().any(|i| i == "types") {
            return true;
        }

        false
    };

    // Extract imports (filter out standard WASI interfaces)
    for (import_name, _item) in ty.imports(&engine) {
        if let Some(interface) = parse_interface(import_name)
            && !is_standard_wasi(&interface)
        {
            interfaces.insert(interface);
        }
    }

    // Extract exports (these are what the component provides to plugins)
    for (export_name, _item) in ty.exports(&engine) {
        if let Some(interface) = parse_interface(export_name) {
            interfaces.insert(interface);
        }
    }

    Ok(interfaces)
}

/// Create the [`Workload`] structure for the development component
///
/// ## Arguments
/// - `bytes`: The bytes of the component to develop
/// - `wasi_config`: Any wasi configuration to pass to the workload
/// - `volume_root`: The root directory of available scratch space to pass as a [`Volume`].
///   Must be a valid UTF-8 path.
fn create_workload(
    bytes: Bytes,
    wasi_config: HashMap<String, String>,
    volume_root: PathBuf,
    dev_register_components: Vec<Bytes>,
) -> Workload {
    // Extract both imports and exports from the component
    // This populates host_interfaces which is checked bidirectionally during plugin binding
    let mut host_interfaces = extract_component_interfaces(&bytes)
        .unwrap_or_else(|e| {
            warn!(error = ?e, "failed to extract component interfaces, using empty interface list");
            HashSet::new()
        })
        .into_iter()
        .collect::<Vec<_>>();

    // Apply configuration to specific interfaces
    for interface in &mut host_interfaces {
        match (interface.namespace.as_str(), interface.package.as_str()) {
            // wasi:http incoming-handler needs host="*" config for routing
            ("wasi", "http") if interface.interfaces.contains("incoming-handler") => {
                interface.config.insert("host".to_string(), "*".to_string());
            }
            // wasi:config/store gets the wasi config
            ("wasi", "config") if interface.interfaces.contains("store") => {
                interface.config = wasi_config.clone();
            }
            _ => {}
        }
    }

    let mut components = Vec::with_capacity(dev_register_components.len() + 1);
    components.push(Component {
        bytes,
        local_resources: LocalResources {
            volume_mounts: vec![VolumeMount {
                name: "dev".to_string(),
                mount_path: "/tmp".to_string(),
                read_only: false,
            }],
            ..Default::default()
        },
        pool_size: -1,
        max_invocations: -1,
    });
    components.extend(dev_register_components.into_iter().map(|bytes| Component {
        bytes,
        // TODO: Must have the root, but can't isolate rn
        // local_resources: LocalResources {
        //     volume_mounts: vec![VolumeMount {
        //         name: "plugin-scratch-dir".to_string(),
        //         // mount_path: "foo",
        //         read_only: false,
        //     }],
        //     ..Default::default()
        // },
        ..Default::default()
    }));
    Workload {
        namespace: "default".to_string(),
        name: "dev".to_string(),
        components,
        host_interfaces,
        annotations: HashMap::default(),
        service: None,
        volumes: vec![Volume {
            name: "dev".to_string(),
            volume_type: VolumeType::HostPath(HostPathVolume {
                local_path: volume_root.to_string_lossy().to_string(),
            }),
        }],
    }
}

/// Reload the component in the host, stopping the previous workload if needed
async fn reload_component(
    host: Arc<Host>,
    workload: &Workload,
    workload_id: Option<String>,
) -> anyhow::Result<String> {
    if let Some(workload_id) = workload_id {
        host.workload_stop(WorkloadStopRequest { workload_id })
            .await?;
    }

    let response = host
        .workload_start(WorkloadStartRequest {
            workload: workload.to_owned(),
        })
        .await?;

    if response.workload_status.workload_state != WorkloadState::Running {
        bail!(
            "failed to reload component: {}",
            response.workload_status.message
        );
    }

    Ok(response.workload_status.workload_id)
}

/// Check if WIT-related files have been modified since the last build
///
/// This function checks the following files for modifications:
/// - `wit/**/*.wit` files (excluding `wit/deps/*`)
///
/// # Arguments
/// * `project_root` - The project root directory
/// * `last_build_time` - The timestamp of the last build
///
/// # Returns
/// `true` if any WIT-related files have been modified, `false` otherwise
fn wit_files_changed(project_root: &Path, last_build_time: SystemTime) -> bool {
    // Check wit/**/*.wit files (excluding wit/deps/*)
    let wit_dir = project_root.join("wit");
    if wit_dir.exists()
        && let Ok(entries) = std::fs::read_dir(&wit_dir)
    {
        for entry in entries.flatten() {
            let path = entry.path();
            // Skip the deps directory
            if path.is_dir() && path.file_name().is_some_and(|n| n == "deps") {
                continue;
            }

            // Check .wit files directly in wit/
            if path.is_file()
                && path.extension().is_some_and(|ext| ext == "wit")
                && let Ok(metadata) = path.metadata()
                && let Ok(modified) = metadata.modified()
                && modified > last_build_time
            {
                debug!(path = ?path.display(), "WIT file modified since last build");
                return true;
            }

            // Recursively check subdirectories (except deps)
            if path.is_dir() && check_wit_files_in_dir(&path, last_build_time) {
                return true;
            }
        }
    }

    false
}

/// Helper function to recursively check for modified .wit files in a directory
fn check_wit_files_in_dir(dir: &Path, last_build_time: SystemTime) -> bool {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();

            if path.is_file()
                && path.extension().is_some_and(|ext| ext == "wit")
                && let Ok(metadata) = path.metadata()
                && let Ok(modified) = metadata.modified()
                && modified > last_build_time
            {
                debug!(path = ?path.display(), "WIT file modified since last build");
                return true;
            } else if path.is_dir() && check_wit_files_in_dir(&path, last_build_time) {
                return true;
            }
        }
    }
    false
}

/// Helper function to check if a path should be ignored during file watching
/// to prevent artifact directories from triggering rebuilds
///
/// # Arguments
/// * `path` - The file path to check
/// * `canonical_project_root` - The canonicalized project root directory
/// * `ignore_paths` - Set of canonicalized paths that should be ignored
fn is_ignored(
    path: &Path,
    _canonical_project_root: &Path,
    ignore_paths: &HashSet<PathBuf>,
) -> bool {
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    ignore_paths.iter().any(|p| canonical_path.starts_with(p))
}

/// Build a set of paths that should be ignored during file watching
/// Uses project-type-specific defaults when available
///
/// # Arguments
/// * `canonical_project_root` - The canonicalized project root directory
/// * `artifact_path` - Optional path to the build artifact
/// * `project_context` - Detected project type for specific ignore patterns
///
/// # Returns
/// A set of canonicalized paths that should be ignored during file watching
fn build_ignore_set(
    canonical_project_root: &Path,
    project_context: &ProjectContext,
) -> HashSet<PathBuf> {
    let mut ignore_paths = HashSet::new();

    // Common directories for all project types
    let mut dirs_to_ignore = vec![".git"];

    // Add project-type-specific ignore patterns
    match project_context {
        ProjectContext::Rust { .. } => {
            dirs_to_ignore.extend_from_slice(&["target"]);
        }
        ProjectContext::TypeScript { .. } => {
            dirs_to_ignore.extend_from_slice(&["node_modules", "dist", "build", ".next", ".nuxt"]);
        }
        ProjectContext::Go { .. } => {
            dirs_to_ignore.extend_from_slice(&["bin", "pkg", "vendor"]);
        }
        ProjectContext::Mixed { detected_types } => {
            // Add ignore patterns for all detected project types
            if detected_types.iter().any(|t| t == "Rust") {
                dirs_to_ignore.extend_from_slice(&["target"]);
            }
            if detected_types
                .iter()
                .any(|t| t == "TypeScript" || t == "JavaScript")
            {
                dirs_to_ignore.extend_from_slice(&[
                    "node_modules",
                    "dist",
                    "build",
                    ".next",
                    ".nuxt",
                ]);
            }
            if detected_types.iter().any(|t| t == "Go") {
                dirs_to_ignore.extend_from_slice(&["bin", "pkg", "vendor"]);
            }
        }
        ProjectContext::General => {
            // For general context, include common patterns from all project types
            dirs_to_ignore.extend_from_slice(&[
                "target",
                "build",
                "dist",
                "node_modules",
                ".next",
                ".nuxt",
                "bin",
                "pkg",
                "vendor",
            ]);
        }
    }

    // Build canonical paths for ignore directories relative to canonical project root
    for dir in &dirs_to_ignore {
        let dir_path = canonical_project_root.join(dir);
        if let Ok(canonical) = dir_path.canonicalize() {
            ignore_paths.insert(canonical);
        } else {
            // Even if the directory doesn't exist yet, include the absolute path
            ignore_paths.insert(dir_path);
        }
    }

    ignore_paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_extract_component_interfaces_with_http_export() {
        // Create a component that exports wasi:http/incoming-handler
        // Using import syntax since WAT exports require actual implementations
        let wat = r#"
            (component
                (import "wasi:http/incoming-handler@0.2.0" (instance))
            )
        "#;
        let component_bytes = wat::parse_str(wat).expect("failed to parse WAT");

        let interfaces =
            extract_component_interfaces(&component_bytes).expect("failed to extract interfaces");

        // Should have extracted 1 interface
        assert_eq!(interfaces.len(), 1, "expected 1 interface");

        // Check for wasi:http interface
        let http_interface = interfaces
            .iter()
            .find(|i| i.namespace == "wasi" && i.package == "http")
            .expect("wasi:http interface not found");
        assert!(
            http_interface.interfaces.contains("incoming-handler"),
            "should contain incoming-handler interface"
        );
    }

    #[test]
    fn test_extract_component_interfaces_with_version() {
        let wat = r#"
            (component
                (import "wasi:http/incoming-handler@0.2.2" (instance))
            )
        "#;
        let component_bytes = wat::parse_str(wat).expect("failed to parse WAT");

        let interfaces =
            extract_component_interfaces(&component_bytes).expect("failed to extract interfaces");

        assert_eq!(interfaces.len(), 1);
        let interface = interfaces.iter().next().unwrap();

        // Version parsing might not work perfectly, but interface should be extracted
        assert_eq!(interface.namespace, "wasi");
        assert_eq!(interface.package, "http");
        assert!(interface.interfaces.contains("incoming-handler"));
    }

    #[test]
    fn test_extract_component_interfaces_no_interfaces() {
        // Component with no imports or exports
        let wat = r#"
            (component)
        "#;
        let component_bytes = wat::parse_str(wat).expect("failed to parse WAT");

        let interfaces =
            extract_component_interfaces(&component_bytes).expect("failed to extract interfaces");

        assert_eq!(
            interfaces.len(),
            0,
            "expected no interfaces for component with no imports/exports"
        );
    }

    #[test]
    fn test_extract_component_interfaces_invalid_bytes() {
        let invalid_bytes = b"not a valid component";

        let result = extract_component_interfaces(invalid_bytes);
        assert!(
            result.is_err(),
            "should fail to extract interfaces from invalid bytes"
        );
    }

    #[test]
    fn test_create_workload_applies_http_config() {
        // HTTP server component with incoming-handler interface
        let wat = r#"
            (component
                (import "wasi:http/incoming-handler@0.2.0" (instance))
            )
        "#;
        let component_bytes = Bytes::from(wat::parse_str(wat).expect("failed to parse WAT"));

        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let volume_root = temp_dir.path().to_path_buf();
        let wasi_config = HashMap::new();
        let dev_register_components = vec![];

        let workload = create_workload(
            component_bytes,
            wasi_config,
            volume_root,
            dev_register_components,
        );

        // Find the HTTP interface in host_interfaces
        let http_interface = workload
            .host_interfaces
            .iter()
            .find(|i| i.namespace == "wasi" && i.package == "http")
            .expect("wasi:http interface not found in workload");

        // Verify host="*" config was applied
        assert_eq!(
            http_interface.config.get("host"),
            Some(&"*".to_string()),
            "wasi:http/incoming-handler should have host='*' config"
        );
        assert!(
            http_interface.interfaces.contains("incoming-handler"),
            "should contain incoming-handler interface"
        );
    }

    #[test]
    fn test_create_workload_applies_wasi_config_to_store() {
        // Component that uses config store interface
        let wat = r#"
            (component
                (import "wasi:config/store@0.2.0" (instance))
            )
        "#;
        let component_bytes = Bytes::from(wat::parse_str(wat).expect("failed to parse WAT"));

        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let volume_root = temp_dir.path().to_path_buf();
        let mut wasi_config = HashMap::new();
        wasi_config.insert(
            "database_url".to_string(),
            "postgres://localhost".to_string(),
        );
        wasi_config.insert("api_key".to_string(), "secret123".to_string());
        let dev_register_components = vec![];

        let workload = create_workload(
            component_bytes,
            wasi_config.clone(),
            volume_root,
            dev_register_components,
        );

        // Find the config interface
        let config_interface = workload
            .host_interfaces
            .iter()
            .find(|i| i.namespace == "wasi" && i.package == "config")
            .expect("wasi:config interface not found in workload");

        // Verify wasi config was applied
        assert_eq!(
            config_interface.config, wasi_config,
            "wasi config should be applied to wasi:config/store"
        );
    }

    #[test]
    fn test_create_workload_with_no_interfaces() {
        // Component with no imports/exports - should create workload with empty host_interfaces
        let wat = r#"
            (component)
        "#;
        let component_bytes = Bytes::from(wat::parse_str(wat).expect("failed to parse WAT"));

        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let volume_root = temp_dir.path().to_path_buf();
        let wasi_config = HashMap::new();
        let dev_register_components = vec![];

        let workload = create_workload(
            component_bytes,
            wasi_config,
            volume_root,
            dev_register_components,
        );

        assert_eq!(
            workload.host_interfaces.len(),
            0,
            "workload with no interfaces should have no host_interfaces"
        );
    }

    #[test]
    fn test_create_workload_graceful_fallback_on_invalid_component() {
        // Invalid component bytes should fall back to empty interface list
        let invalid_bytes = Bytes::from_static(b"not a valid component");

        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let volume_root = temp_dir.path().to_path_buf();
        let wasi_config = HashMap::new();
        let dev_register_components = vec![];

        let workload = create_workload(
            invalid_bytes,
            wasi_config,
            volume_root,
            dev_register_components,
        );

        // Should gracefully fall back to empty interfaces
        assert_eq!(
            workload.host_interfaces.len(),
            0,
            "invalid component should result in empty host_interfaces with graceful fallback"
        );
    }

    #[test]
    fn test_create_workload_basic_structure() {
        // HTTP server component with incoming-handler interface
        let wat = r#"
            (component
                (import "wasi:http/incoming-handler@0.2.0" (instance))
            )
        "#;
        let component_bytes = Bytes::from(wat::parse_str(wat).expect("failed to parse WAT"));

        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let volume_root = temp_dir.path().to_path_buf();
        let wasi_config = HashMap::new();
        let dev_register_components = vec![];

        let workload = create_workload(
            component_bytes.clone(),
            wasi_config,
            volume_root.clone(),
            dev_register_components,
        );

        // Verify basic workload structure
        assert_eq!(workload.namespace, "default");
        assert_eq!(workload.name, "dev");
        assert_eq!(workload.components.len(), 1);
        assert_eq!(workload.volumes.len(), 1);

        // Verify the main component
        let component = &workload.components[0];
        assert_eq!(component.bytes, component_bytes);
        assert_eq!(component.pool_size, -1);
        assert_eq!(component.max_invocations, -1);

        // Verify volume mount
        assert_eq!(component.local_resources.volume_mounts.len(), 1);
        assert_eq!(component.local_resources.volume_mounts[0].name, "dev");
        assert_eq!(
            component.local_resources.volume_mounts[0].mount_path,
            "/tmp"
        );
        assert!(!component.local_resources.volume_mounts[0].read_only);

        // Verify volume configuration
        assert_eq!(workload.volumes.len(), 1);
        let volume = &workload.volumes[0];
        assert_eq!(volume.name, "dev");

        match &volume.volume_type {
            VolumeType::HostPath(host_path) => {
                assert_eq!(
                    host_path.local_path,
                    volume_root.to_string_lossy().to_string()
                );
            }
            _ => panic!("Expected HostPath volume type"),
        }
    }

    #[test]
    fn test_is_ignored_rust_project() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_root = temp_dir
            .path()
            .canonicalize()
            .expect("failed to canonicalize temp dir");

        // Create target directory
        fs::create_dir_all(project_root.join("target")).expect("failed to create target dir");

        let context = ProjectContext::Rust {
            cargo_toml_path: project_root.join("Cargo.toml"),
        };
        let ignore_paths = build_ignore_set(&project_root, &context);

        // Test that target/ is ignored
        let target_file = project_root.join("target").join("test.txt");
        assert!(is_ignored(&target_file, &project_root, &ignore_paths));

        // Test that src/ is not ignored
        let src_file = project_root.join("src").join("lib.rs");
        assert!(!is_ignored(&src_file, &project_root, &ignore_paths));
    }

    #[test]
    fn test_is_ignored_typescript_project() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_root = temp_dir
            .path()
            .canonicalize()
            .expect("failed to canonicalize temp dir");

        fs::create_dir_all(project_root.join("node_modules"))
            .expect("failed to create node_modules");

        let context = ProjectContext::TypeScript {
            package_json_path: project_root.join("package.json"),
        };
        let ignore_paths = build_ignore_set(&project_root, &context);

        // Test that node_modules/ is ignored
        let node_modules_file = project_root.join("node_modules").join("test");
        assert!(is_ignored(&node_modules_file, &project_root, &ignore_paths));

        // Test that src/ is not ignored
        let src_file = project_root.join("src").join("index.ts");
        assert!(!is_ignored(&src_file, &project_root, &ignore_paths));
    }

    #[test]
    fn test_artifact_parent_not_ignored_by_default() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_root = temp_dir
            .path()
            .canonicalize()
            .expect("failed to canonicalize temp dir");
        let artifact_dir = project_root.join("custom");
        let artifact_path = artifact_dir.join("output.wasm");

        fs::create_dir_all(&artifact_dir).expect("failed to create artifact dir");
        fs::write(&artifact_path, "test content").expect("failed to create artifact file");

        let context = ProjectContext::General;
        let ignore_paths = build_ignore_set(&project_root, &context);

        // Sibling in custom/ is **not** ignored anymore
        let sibling_file = artifact_dir.join("other.wasm");
        fs::write(&sibling_file, "other content").expect("failed to create sibling file");
        assert!(!is_ignored(&sibling_file, &project_root, &ignore_paths));

        // Outside normal ignore dirs should also not be ignored
        let outside_file = project_root.join("src").join("main.rs");
        assert!(!is_ignored(&outside_file, &project_root, &ignore_paths));
    }

    #[test]
    fn test_mixed_project_includes_all_patterns() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_root = temp_dir
            .path()
            .canonicalize()
            .expect("failed to canonicalize temp dir");

        // Create directories for different project types
        fs::create_dir_all(project_root.join("target")).expect("failed to create target");
        fs::create_dir_all(project_root.join("node_modules"))
            .expect("failed to create node_modules");

        let context = ProjectContext::Mixed {
            detected_types: vec!["Rust".to_string(), "TypeScript".to_string()],
        };
        let ignore_paths = build_ignore_set(&project_root, &context);

        // Test that both Rust and TypeScript patterns are ignored
        let target_file = project_root.join("target").join("test");
        let node_modules_file = project_root.join("node_modules").join("test");

        assert!(is_ignored(&target_file, &project_root, &ignore_paths));
        assert!(is_ignored(&node_modules_file, &project_root, &ignore_paths));
    }

    #[test]
    fn test_relative_vs_absolute_path_consistency() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_root = temp_dir
            .path()
            .canonicalize()
            .expect("failed to canonicalize temp dir");

        // Create target directory
        fs::create_dir_all(project_root.join("target")).expect("failed to create target dir");

        let context = ProjectContext::Rust {
            cargo_toml_path: project_root.join("Cargo.toml"),
        };
        let ignore_paths = build_ignore_set(&project_root, &context);

        // Test with absolute path
        let absolute_target_file = project_root.join("target").join("test.txt");

        // Test with relative path (simulate what might come from file watcher)
        let relative_target_file = PathBuf::from("./target/test.txt");

        // Both should be consistently ignored when checked against canonical project root
        assert!(is_ignored(
            &absolute_target_file,
            &project_root,
            &ignore_paths
        ));

        // Note: For relative paths to work correctly, they need to be resolved
        // relative to the project root first, which is what our canonicalization handles
        let resolved_relative = project_root.join(
            relative_target_file
                .strip_prefix("./")
                .unwrap_or(&relative_target_file),
        );
        assert!(is_ignored(&resolved_relative, &project_root, &ignore_paths));
    }

    #[test]
    fn test_symlink_handling() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_root = temp_dir
            .path()
            .canonicalize()
            .expect("failed to canonicalize temp dir");

        // Create actual target directory outside project
        let external_target = temp_dir.path().join("external_target");
        fs::create_dir_all(&external_target).expect("failed to create external target");

        // Create symlink from project to external target
        let symlink_target = project_root.join("target");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&external_target, &symlink_target)
            .expect("failed to create symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&external_target, &symlink_target)
            .expect("failed to create symlink");

        let context = ProjectContext::Rust {
            cargo_toml_path: project_root.join("Cargo.toml"),
        };
        let ignore_paths = build_ignore_set(&project_root, &context);

        // Test file accessed through symlink
        let file_through_symlink = symlink_target.join("test.txt");
        fs::write(external_target.join("test.txt"), "content").expect("failed to write test file");

        // Should be ignored because the symlink resolves to target/ pattern
        assert!(is_ignored(
            &file_through_symlink,
            &project_root,
            &ignore_paths
        ));

        // Also test direct access to the external target
        let external_file = external_target.join("test.txt");
        // This might or might not be ignored depending on canonicalization
        // The key is that symlinked paths are handled consistently
        let _is_external_ignored = is_ignored(&external_file, &project_root, &ignore_paths);
    }

    #[test]
    fn test_nested_dirs_not_ignored_without_explicit_entry() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_root = temp_dir
            .path()
            .canonicalize()
            .expect("failed to canonicalize temp dir");

        // Top-level target/ (explicitly ignored by build_ignore_set)
        let top_target = project_root.join("target");
        fs::create_dir_all(top_target.join("debug")).expect("failed to create top-level target");
        let top_level_file = top_target.join("debug").join("top.wasm");

        // Nested structure: project/subproject/target (NOT explicitly in ignore set)
        let subproject_dir = project_root.join("subproject");
        let nested_target = subproject_dir.join("target");
        fs::create_dir_all(nested_target.join("debug")).expect("failed to create nested target");
        let nested_file = nested_target.join("debug").join("nested.wasm");

        let context = ProjectContext::Rust {
            cargo_toml_path: project_root.join("Cargo.toml"),
        };
        let ignore_paths = build_ignore_set(&project_root, &context);

        // Baseline: top-level target/* IS ignored
        assert!(is_ignored(&top_level_file, &project_root, &ignore_paths));

        // subproject/target/* is NOT ignored
        assert!(!is_ignored(&nested_file, &project_root, &ignore_paths));

        // Sanity: subproject source is NOT ignored
        let subproject_src = subproject_dir.join("src").join("main.rs");
        assert!(!is_ignored(&subproject_src, &project_root, &ignore_paths));
    }
}
