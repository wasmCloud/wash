use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context as _, ensure};
use base64::Engine;
use clap::Args;
use etcetera::AppStrategy as _;
use hyper::server::conn::http1;
use indicatif::{ProgressBar, ProgressStyle};
use notify::{
    Event as NotifyEvent, RecursiveMode, Watcher,
    event::{EventKind, ModifyKind},
};
use tokio::{
    net::TcpListener,
    select,
    sync::{RwLock, mpsc},
};
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
        doctor::{check_project_specific_tools, detect_project_context},
    },
    component_build::BuildConfig,
    config::{Config, load_config},
    dev::DevPluginManager,
    plugin::list_plugins,
    runtime::{
        Ctx, bindings::plugin::exports::wasmcloud::wash::plugin::HookType, prepare_component_dev,
    },
};

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

        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap(),
        );
        spinner.set_message("Building component...");
        spinner.enable_steady_tick(Duration::from_millis(100));

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
                spinner.finish_with_message("✅ Component built successfully");
                warn!(path = ?build_result.artifact_path, "component built successfully, but artifact path in config is different");
                // Ensure the artifact path is set in the config
                build_result.artifact_path
            }
            // Use the build result artifact path if the config does not specify one
            Ok(build_result) => {
                spinner.finish_with_message("✅ Component built successfully");
                debug!(path = ?build_result.artifact_path, "component built successfully, using as artifact path");
                build_result.artifact_path
            }
            Err(e) => {
                spinner.finish_with_message("❌ Build failed");
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

        // TODO(#19): Only spawn the server if the component exports wasi:http
        tokio::spawn(async move {
            if let Err(e) = server(&mut component_rx, address, runtime_config, blobstore_root).await
            {
                error!(err = ?e,"error running http server for dev");
            }
        });

        // Enable/disable watching to prevent having the output artifact trigger a rebuild
        // This starts as true to prevent a rebuild on the first run
        let pause_watch = Arc::new(AtomicBool::new(true));
        let watcher_paused = pause_watch.clone();
        let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
        let (reload_tx, mut reload_rx) = mpsc::channel::<()>(1);

        // let project_path_notify = self.project_dir.clone();
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
                    // TODO(#20): ensure we ignore the artifact dirs
                    // Ensure that paths that take place in ignored directories don't trigger a reload
                    // This is primarily here to avoid recursively triggering reloads for files that are
                    // generated by the build process.
                    // if paths.iter().any(|p| {
                    //     p.strip_prefix(project_path_notify.as_path())
                    //         .is_ok_and(|p| {
                    //             cmd.ignore_dirs.iter().any(|ignore| p.starts_with(ignore))
                    //         })
                    // }) {
                    //     return;
                    // }
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

        watcher.watch(&self.project_dir, RecursiveMode::Recursive)?;
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
        info!(address = self.address, "listening for HTTP requests");

        // Create a single spinner for the entire dev session
        let dev_spinner = ProgressBar::new_spinner();
        dev_spinner.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap(),
        );

        loop {
            info!("watching for file changes (press Ctrl+c to stop)...");
            select! {
                // Process a file change/reload
                _ = reload_rx.recv() => {
                    pause_watch.store(true, Ordering::SeqCst);

                    dev_spinner.set_message("Rebuilding component...");
                    dev_spinner.enable_steady_tick(Duration::from_millis(100));

                    info!("rebuilding component after file changed ...");

                    // TODO(IMPORTANT): ensure that this calls the build pre-post hooks
                    // TODO(#21): Skip wit fetch if no .wit change
                    // TODO(#22): Typescript: Skip install if no package.json change
                    let rebuild_result = dev_spinner.suspend(|| async {
                        build_component(
                            &self.project_dir,
                            ctx,
                            &config,
                        ).await
                    }).await;

                    match rebuild_result {
                        Ok(build_result) => {
                            // Use the new artifact path from the build result
                            let artifact_path = build_result.artifact_path;
                            dev_spinner.set_message("Deploying rebuilt component...");
                            info!(path = %artifact_path.display(), "component rebuilt successfully");

                            let wasm_bytes = tokio::fs::read(&artifact_path)
                                .await
                                .context("failed to read artifact file")?;
                            let component = prepare_component_dev(&ctx.runtime, &wasm_bytes, plugin_manager.clone().clear_instances())
                                .await
                                .context("failed to prepare component")?;
                            component_tx.send_replace(Arc::new(component));

                            dev_spinner.finish_with_message("✅ Component rebuilt and deployed successfully");
                            dev_spinner.reset();

                            // Avoid jitter with reloads by pausing the watcher for a short time
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            // Make sure that the reload channel is empty before unpausing the watcher
                            let _ = reload_rx.try_recv();
                            pause_watch.store(false, Ordering::SeqCst);
                        }
                        Err(e) => {
                            dev_spinner.finish_with_message("❌ Rebuild failed");
                            dev_spinner.reset();
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

/// Starts the development HTTP server, listening for incoming requests
/// and serving them using the provided [`CustomCtxComponent`]. The component
/// is provided via a `tokio::sync::watch::Receiver`, allowing it to be
/// updated dynamically (e.g. on a rebuild).
async fn server(
    rx: &mut tokio::sync::watch::Receiver<Arc<CustomCtxComponent<Ctx>>>,
    address: String,
    runtime_config: HashMap<String, String>,
    blobstore_root: PathBuf,
) -> anyhow::Result<()> {
    // Prepare our server state and start listening for connections.
    let mut component = rx.borrow_and_update().to_owned();
    let listener = TcpListener::bind(&address).await?;
    debug!("listening on {}", listener.local_addr()?);
    loop {
        let blobstore_root = blobstore_root.clone();
        let runtime_config = runtime_config.clone();
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
                debug!(addr = ?addr, "serving new client");

                tokio::spawn(async move {
                    let component = component.clone();
                    if let Err(e) = http1::Builder::new()
                        .keep_alive(true)
                        .serve_connection(
                            TokioIo::new(client),
                            hyper::service::service_fn(move |req| {
                                let component = component.clone();
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
                                    .build();
                                async move { component.handle_request(Some(ctx), req).await }
                            }),
                        )
                        .await
                    {
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
    ) -> anyhow::Result<hyper::Response<HyperOutgoingBody>>;
}
impl HandleRequest for CustomCtxComponent<Ctx> {
    async fn handle_request(
        &self,
        ctx: Option<Ctx>,
        req: hyper::Request<hyper::body::Incoming>,
    ) -> anyhow::Result<hyper::Response<HyperOutgoingBody>> {
        // Create per-http-request state within a `Store` and prepare the
        // initial resources passed to the `handle` function.
        let ctx = ctx.unwrap_or_default();
        let mut store = self.new_store(ctx);
        let pre = self.instance_pre().clone();
        handle_request(store.as_context_mut(), pre, req).await
    }
}

pub async fn handle_request<'a>(
    mut store: StoreContextMut<'a, Ctx>,
    pre: InstancePre<Ctx>,
    req: hyper::Request<hyper::body::Incoming>,
) -> anyhow::Result<hyper::Response<HyperOutgoingBody>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let req = store.data_mut().new_incoming_request(Scheme::Http, req)?;
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
