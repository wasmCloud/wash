use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context as _, ensure};
use base64::Engine;
use clap::Args;
use etcetera::AppStrategy as _;
use hyper::server::conn::http1;
use notify::{
    Event as NotifyEvent, RecursiveMode, Watcher,
    event::{EventKind, ModifyKind},
};
use rustls::{ServerConfig, pki_types::CertificateDer};
use rustls_pemfile::{certs, private_key};
use tokio::{
    net::TcpListener,
    select,
    sync::{RwLock, mpsc},
};
use tokio_rustls::TlsAcceptor;
use tracing::{debug, error, info, trace, warn};
use wasmcloud_runtime::component::CustomCtxComponent;
use wasmtime::{AsContextMut, StoreContextMut, component::InstancePre};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder};
use wasmtime_wasi_http::{
    WasiHttpView as _,
    bindings::{ProxyPre, http::types::Scheme},
    body::HyperOutgoingBody,
    io::TokioIo,
};

use crate::{
    cli::{
        CliCommand, CliContext, CommandOutput,
        component_build::build_component,
        doctor::{ProjectContext, check_project_specific_tools, detect_project_context},
    },
    component_build::BuildConfig,
    config::{Config, load_config},
    dev::DevPluginManager,
    plugin::list_plugins,
    runtime::{
        Ctx, bindings::plugin::exports::wasmcloud::wash::plugin::HookType, prepare_component_dev,
    },
    wit::change_detection::DependencyTracker,
};

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

#[derive(Debug, Clone, Args)]
pub struct DevCommand {
    /// The path to the project directory
    #[clap(name = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,

    /// The path to the built Wasm file to be used in development
    #[clap(long = "artifact-path")]
    pub artifact_path: Option<PathBuf>,

    /// The address on which the HTTP server will listen
    #[clap(long = "address", default_value = "0.0.0.0:8000")]
    pub address: String,

    /// Configuration values to use for `wasi:config/runtime` in the form of `key=value` pairs.
    #[clap(long = "runtime-config", value_delimiter = ',')]
    pub runtime_config: Vec<String>,

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
            // Override the artifact path with the one provided in the command line
            Some(Config {
                build: Some(BuildConfig {
                    artifact_path: self.artifact_path.clone(),
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

        let artifact_path = match build_component(&self.project_dir, ctx, &config).await {
            // Edge case where the build was successful, but the artifact path in the config is different
            // than the one returned by the build process.
            Ok(build_result)
                if config
                    .build
                    .as_ref()
                    .and_then(|b| b.artifact_path.as_ref())
                    .is_some_and(|p| p != &build_result.artifact_path) =>
            {
                warn!(path = ?build_result.artifact_path, "component built successfully, but artifact path in config is different");
                // Ensure the artifact path is set in the config
                build_result.artifact_path
            }
            // Use the build result artifact path if the config does not specify one
            Ok(build_result) => {
                debug!(path = ?build_result.artifact_path, "component built successfully, using as artifact path");
                build_result.artifact_path
            }
            Err(e) => {
                // TODO(#18): Support continuing, npm start works like that.
                error!("failed to build component, will not start dev session");
                error!("{e}");
                return Err(e);
            }
        };

        // Deploy to local runtime
        let wasm_bytes = tokio::fs::read(&artifact_path)
            .await
            .context("failed to read artifact file")?;

        // Call pre-hooks before starting dev session
        let pre_context = HashMap::new(); // Empty context for pre-hooks
        let pre_runtime_context = Arc::new(RwLock::new(pre_context));
        match ctx
            .call_pre_hooks(pre_runtime_context, HookType::BeforeDev)
            .await
        {
            Ok(_) => {}
            Err(e) => {
                error!("pre-hook execution failed, will not start dev session");
                error!("{e}");
                return Err(e);
            }
        }

        let mut plugin_manager = DevPluginManager::default();
        let plugins = match list_plugins(ctx.runtime(), ctx.data_dir()).await {
            Ok(plugins) => plugins
                .into_iter()
                .filter(|plugin| {
                    // Only register plugins that have the dev-hook
                    plugin.metadata.hooks.contains(&HookType::DevRegister)
                })
                .collect(),
            Err(e) => {
                warn!(err = ?e, "failed to find plugins, continuing without plugins");
                vec![]
            }
        };

        for plugin in plugins {
            let name = plugin.metadata.name.clone();
            let version = plugin.metadata.version.clone();
            if let Err(e) = plugin_manager.register_plugin(plugin) {
                error!(
                    err = ?e,
                    name,
                    version,
                    "failed to register plugin, continuing without plugin"
                );
            } else {
                debug!(name, version, "registered plugin");
            }
        }

        let plugin_manager = Arc::new(plugin_manager);

        // Prepare the component for development
        let (component_tx, mut component_rx) =
            tokio::sync::watch::channel::<Arc<CustomCtxComponent<Ctx>>>(Arc::new(
                prepare_component_dev(&ctx.runtime, &wasm_bytes, plugin_manager.clone())
                    .await
                    .context("failed to prepare component for development")?,
            ));

        // Run the HTTP server in a background task
        let address = self.address.clone();
        let runtime_config = self
            .runtime_config
            .clone()
            .into_iter()
            .filter_map(|orig| match orig.split_once('=') {
                Some((k, v)) => Some((k.to_string(), v.to_string())),
                None => {
                    warn!(key = orig, "runtime config key without value, skipping");
                    None
                }
            })
            .collect::<HashMap<String, String>>();

        let blobstore_root = self
            .blobstore_root
            .clone()
            .unwrap_or_else(|| ctx.data_dir().join("dev_blobstore"));
        // Ensure the blobstore root directory exists
        if !blobstore_root.exists() {
            tokio::fs::create_dir_all(&blobstore_root)
                .await
                .context("failed to create blobstore root directory")?;
        }
        debug!(path = ?blobstore_root.display(), "using blobstore root directory");

        // Load TLS configuration if cert and key are provided
        let tls_acceptor =
            if let (Some(cert_path), Some(key_path)) = (&self.tls_cert, &self.tls_key) {
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

                let tls_config = load_tls_config(cert_path, key_path, self.tls_ca.as_deref())
                    .await
                    .context("Failed to load TLS configuration")?;

                debug!("TLS configured - server will use HTTPS");
                Some(TlsAcceptor::from(Arc::new(tls_config)))
            } else if self.tls_cert.is_some() || self.tls_key.is_some() {
                // If only one of cert/key is provided, that's an error
                ensure!(
                    false,
                    "Both --tls-cert and --tls-key must be provided for HTTPS support"
                );
                None
            } else {
                debug!("No TLS configuration provided - server will use HTTP");
                None
            };

        // Determine protocol before moving tls_acceptor
        let protocol = if tls_acceptor.is_some() {
            "https"
        } else {
            "http"
        };

        // TODO(#19): Only spawn the server if the component exports wasi:http
        let background_processes = ctx.background_processes.clone();
        tokio::spawn(async move {
            if let Err(e) = server(
                &mut component_rx,
                address,
                runtime_config,
                blobstore_root,
                background_processes,
                tls_acceptor,
            )
            .await
            {
                error!(err = ?e,"error running http server for dev");
            }
        });

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

        // Initialize dependency tracker for optimizing rebuild performance
        let mut dependency_tracker = DependencyTracker::new();
        debug!("initialized dependency tracker for development session");

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

                    // Check if dependency change detection is disabled in config
                    let change_detection_disabled = config.wit.as_ref()
                        .map(|w| w.disable_change_detection)
                        .unwrap_or(false);

                    let dependencies_changed = if change_detection_disabled {
                        debug!("dependency change detection disabled, forcing full fetch");
                        true
                    } else {
                        dependency_tracker.check_dependencies_changed(&self.project_dir)
                            .await.unwrap_or(true) // Default to true if check fails
                    };

                    let build_config = if dependencies_changed {
                        info!("dependencies changed, fetching all dependencies...");
                        config.clone()
                    } else {
                        info!("dependencies unchanged, skipping fetch for faster build...");
                        // Create a modified config with skip_fetch = true
                        let mut modified_config = config.clone();
                        if let Some(wit_config) = &mut modified_config.wit {
                            wit_config.skip_fetch = true;
                        } else {
                            modified_config.wit = Some(crate::wit::WitConfig {
                                skip_fetch: true,
                                ..Default::default()
                            });
                        }
                        modified_config
                    };

                    // TODO(IMPORTANT): ensure that this calls the build pre-post hooks
                    // Dependency changes are now automatically handled above
                    let rebuild_result = build_component(
                        &self.project_dir,
                        ctx,
                        &build_config,
                    ).await;

                    match rebuild_result {
                        Ok(build_result) => {
                            // Use the new artifact path from the build result
                            let artifact_path = build_result.artifact_path;
                            info!(path = %artifact_path.display(), "component rebuilt successfully");

                            info!("deploying rebuilt component ...");
                            let wasm_bytes = tokio::fs::read(&artifact_path)
                                .await
                                .context("failed to read artifact file")?;
                            let component = prepare_component_dev(&ctx.runtime, &wasm_bytes, plugin_manager.clone().clear_instances())
                                .await
                                .context("failed to prepare component")?;
                            component_tx.send_replace(Arc::new(component));

                            // Dependency tracker is automatically updated during the check
                            debug!("dependency tracking updated after successful build");

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

        // Call post-hooks with component bytes context
        // Base64 encode the bytes since context only supports HashMap<String, String>
        let component_bytes_b64 = base64::engine::general_purpose::STANDARD.encode(&wasm_bytes);
        let mut post_context = HashMap::new();
        post_context.insert(
            "dev.component_bytes_base64".to_string(),
            component_bytes_b64,
        );
        let post_runtime_context = Arc::new(RwLock::new(post_context));
        ctx.call_post_hooks(post_runtime_context, HookType::AfterDev)
            .await?;

        Ok(CommandOutput::ok(
            "Development command executed successfully".to_string(),
            None,
        ))
    }
}

/// Load TLS configuration from certificate and key files
async fn load_tls_config(
    cert_path: &Path,
    key_path: &Path,
    ca_path: Option<&Path>,
) -> anyhow::Result<ServerConfig> {
    // Load certificate chain
    let cert_data = tokio::fs::read(cert_path)
        .await
        .with_context(|| format!("Failed to read certificate file: {}", cert_path.display()))?;
    let mut cert_reader = std::io::Cursor::new(cert_data);
    let cert_chain: Vec<CertificateDer<'static>> = certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("Failed to parse certificate file: {}", cert_path.display()))?;

    ensure!(
        !cert_chain.is_empty(),
        "No certificates found in file: {}",
        cert_path.display()
    );

    // Load private key
    let key_data = tokio::fs::read(key_path)
        .await
        .with_context(|| format!("Failed to read private key file: {}", key_path.display()))?;
    let mut key_reader = std::io::Cursor::new(key_data);
    let key = private_key(&mut key_reader)
        .with_context(|| format!("Failed to parse private key file: {}", key_path.display()))?
        .ok_or_else(|| anyhow::anyhow!("No private key found in file: {}", key_path.display()))?;

    // Create rustls server config
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .with_context(|| "Failed to create TLS configuration")?;

    // If CA is provided, configure client certificate verification
    if let Some(ca_path) = ca_path {
        let ca_data = tokio::fs::read(ca_path)
            .await
            .with_context(|| format!("Failed to read CA file: {}", ca_path.display()))?;
        let mut ca_reader = std::io::Cursor::new(ca_data);
        let ca_certs: Vec<CertificateDer<'static>> = certs(&mut ca_reader)
            .collect::<Result<Vec<_>, _>>()
            .with_context(|| format!("Failed to parse CA file: {}", ca_path.display()))?;

        ensure!(
            !ca_certs.is_empty(),
            "No CA certificates found in file: {}",
            ca_path.display()
        );

        // Note: Client certificate verification configuration would go here
        // For now, we'll keep it simple without client cert verification
        debug!("CA certificate loaded, but client certificate verification not yet implemented");
    }

    Ok(config)
}

/// Starts the development HTTP server, listening for incoming requests
/// and serving them using the provided [`CustomCtxComponent`]. The component
/// is provided via a `tokio::sync::watch::Receiver`, allowing it to be
/// updated dynamically (e.g. on a rebuild).
async fn server(
    rx: &mut tokio::sync::watch::Receiver<Arc<CustomCtxComponent<Ctx>>>,
    address: String,
    runtime_config: HashMap<String, String>,
    blobstore_root: PathBuf,
    background_processes: Arc<RwLock<Vec<tokio::process::Child>>>,
    tls_acceptor: Option<TlsAcceptor>,
) -> anyhow::Result<()> {
    // Prepare our server state and start listening for connections.
    let mut component = rx.borrow_and_update().to_owned();
    let listener = TcpListener::bind(&address).await?;
    loop {
        let blobstore_root = blobstore_root.clone();
        let runtime_config = runtime_config.clone();
        let background_processes = background_processes.clone();
        select! {
            // If the component changed, replace the current one
            _ = rx.changed() => {
                // If the channel has changed, we need to update the component
                component = rx.borrow_and_update().to_owned();
                debug!("Component updated in main loop");
            }
            // Accept a TCP connection and serve all of its requests in a separate
            // tokio task. Note that for now this only works with HTTP/1.1.
            Ok((client, addr)) = listener.accept() => {
                let component = component.clone();
                let background_processes = background_processes.clone();
                let tls_acceptor = tls_acceptor.clone();
                debug!(addr = ?addr, "serving new client");

                tokio::spawn(async move {
                    let component = component.clone();

                    // Determine the scheme based on whether TLS is configured
                    let scheme = if tls_acceptor.is_some() {
                        Scheme::Https
                    } else {
                        Scheme::Http
                    };

                    // Handle TLS if configured
                    let service = hyper::service::service_fn(move |req| {
                        let component = component.clone();
                        let background_processes = background_processes.clone();
                        let scheme = scheme.clone();
                        let wasi_ctx = match WasiCtxBuilder::new()
                            .preopened_dir(&blobstore_root, "/dev", DirPerms::all(), FilePerms::all())
                        {
                            Ok(ctx) => ctx.build(),
                            Err(e) => {
                                error!(err = ?e, "failed to create WASI context with preopened dir");
                                WasiCtxBuilder::new().build()
                            }
                        };
                        let ctx = Ctx::builder()
                            .with_wasi_ctx(wasi_ctx)
                            .with_runtime_config(runtime_config.clone())
                            .with_background_processes(background_processes)
                            .build();
                        async move { component.handle_request(Some(ctx), req, scheme).await }
                    });

                    let result = if let Some(acceptor) = tls_acceptor {
                        // Handle HTTPS connection
                        match acceptor.accept(client).await {
                            Ok(tls_stream) => {
                                http1::Builder::new()
                                    .keep_alive(true)
                                    .serve_connection(TokioIo::new(tls_stream), service)
                                    .await
                            }
                            Err(e) => {
                                error!(addr = ?addr, err = ?e, "TLS handshake failed");
                                return;
                            }
                        }
                    } else {
                        // Handle HTTP connection
                        http1::Builder::new()
                            .keep_alive(true)
                            .serve_connection(TokioIo::new(client), service)
                            .await
                    };

                    if let Err(e) = result {
                        error!(addr = ?addr, err = ?e, "error serving client");
                    }
                });
            }
        }
    }
}

/// Simple trait for handling an HTTP request, used primarily to extend the
/// `CustomCtxComponent` with a method that can handle HTTP requests
trait HandleRequest {
    async fn handle_request(
        &self,
        ctx: Option<Ctx>,
        req: hyper::Request<hyper::body::Incoming>,
        scheme: Scheme,
    ) -> anyhow::Result<hyper::Response<HyperOutgoingBody>>;
}
impl HandleRequest for CustomCtxComponent<Ctx> {
    async fn handle_request(
        &self,
        ctx: Option<Ctx>,
        req: hyper::Request<hyper::body::Incoming>,
        scheme: Scheme,
    ) -> anyhow::Result<hyper::Response<HyperOutgoingBody>> {
        // Create per-http-request state within a `Store` and prepare the
        // initial resources passed to the `handle` function.
        let ctx = ctx.unwrap_or_default();
        let mut store = self.new_store(ctx);
        let pre = self.instance_pre().clone();
        handle_request(store.as_context_mut(), pre, req, scheme).await
    }
}

pub async fn handle_request<'a>(
    mut store: StoreContextMut<'a, Ctx>,
    pre: InstancePre<Ctx>,
    req: hyper::Request<hyper::body::Incoming>,
    scheme: Scheme,
) -> anyhow::Result<hyper::Response<HyperOutgoingBody>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let req = store.data_mut().new_incoming_request(scheme, req)?;
    let out = store.data_mut().new_response_outparam(sender)?;
    let pre = ProxyPre::new(pre).context("failed to instantiate proxy pre")?;

    // Run the http request itself in a separate task so the task can
    // optionally continue to execute beyond after the initial
    // headers/response code are sent.
    // let task: JoinHandle<anyhow::Result<()>> = tokio::spawn(async move {
    let proxy = pre.instantiate_async(&mut store).await?;

    proxy
        .wasi_http_incoming_handler()
        .call_handle(&mut store, req, out)
        .await?;

    // Ok(())
    // });

    match receiver.await {
        // If the client calls `response-outparam::set` then one of these
        // methods will be called.
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(e)) => Err(e.into()),

        // Otherwise the `sender` will get dropped along with the `Store`
        // meaning that the oneshot will get disconnected and here we can
        // inspect the `task` result to see what happened
        Err(e) => {
            error!(err = ?e, "error receiving http response");
            Err(anyhow::anyhow!(
                "oneshot channel closed but no response was sent"
            ))
            // Err(match task.await {
            //     Ok(Ok(())) => {
            //         anyhow::anyhow!("oneshot channel closed but no response was sent")
            //     }
            //     Ok(Err(e)) => e,
            //     Err(e) => {
            //         anyhow::anyhow!("failed to await task for handling HTTP request: {e}")
            //     }
            // })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

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

    #[cfg(test)]
    #[tokio::test]
    async fn test_tls_config_loading() {
        // Generate self-signed certificate for testing
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("failed to generate self-signed cert");

        // Create temp directory for cert files
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let cert_path = temp_dir.path().join("cert.pem");
        let key_path = temp_dir.path().join("key.pem");

        // Write certificate and key to files
        let cert_pem = cert.cert.pem();
        tokio::fs::write(&cert_path, cert_pem.as_bytes())
            .await
            .expect("failed to write cert");

        let key_pem = cert.key_pair.serialize_pem();
        tokio::fs::write(&key_path, key_pem.as_bytes())
            .await
            .expect("failed to write key");

        // Test loading TLS configuration
        let tls_config = load_tls_config(&cert_path, &key_path, None).await;
        assert!(
            tls_config.is_ok(),
            "Failed to load TLS config: {:?}",
            tls_config.err()
        );

        // Verify the TLS config can be used to create an acceptor
        let config = tls_config.unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));

        // Basic validation that acceptor was created successfully
        // Since TlsAcceptor doesn't implement Debug, just verify it's created
        let _ = acceptor;
    }

    #[cfg(test)]
    #[tokio::test]
    async fn test_tls_server_scheme_detection() {
        // Generate self-signed certificate
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("failed to generate self-signed cert");

        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let cert_path = temp_dir.path().join("cert.pem");
        let key_path = temp_dir.path().join("key.pem");

        let cert_pem = cert.cert.pem();
        tokio::fs::write(&cert_path, cert_pem.as_bytes())
            .await
            .expect("failed to write cert");

        let key_pem = cert.key_pair.serialize_pem();
        tokio::fs::write(&key_path, key_pem.as_bytes())
            .await
            .expect("failed to write key");

        // Load TLS configuration
        let tls_config = load_tls_config(&cert_path, &key_path, None)
            .await
            .expect("failed to load TLS config");
        let tls_acceptor = Some(TlsAcceptor::from(Arc::new(tls_config)));

        // Test that scheme is correctly set to HTTPS when TLS is configured
        assert!(tls_acceptor.is_some());

        // Verify the scheme would be HTTPS
        let scheme = if tls_acceptor.is_some() {
            Scheme::Https
        } else {
            Scheme::Http
        };
        assert_eq!(format!("{:?}", scheme), "Scheme::Https");

        // Test without TLS
        let no_tls_acceptor: Option<TlsAcceptor> = None;
        let scheme = if no_tls_acceptor.is_some() {
            Scheme::Https
        } else {
            Scheme::Http
        };
        assert_eq!(format!("{:?}", scheme), "Scheme::Http");
    }

    #[cfg(test)]
    #[tokio::test]
    async fn test_tls_config_validation() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let nonexistent_cert = temp_dir.path().join("nonexistent.pem");
        let nonexistent_key = temp_dir.path().join("nonexistent_key.pem");

        // Test loading with non-existent files should fail
        let result = load_tls_config(&nonexistent_cert, &nonexistent_key, None).await;
        assert!(result.is_err(), "Should fail with non-existent files");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to read certificate file")
        );
    }
}
