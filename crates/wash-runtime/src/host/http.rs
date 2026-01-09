//! HTTP server plugin for handling incoming HTTP requests.
//!
//! This plugin implements the `wasi:http/incoming-handler` interface, allowing
//! WebAssembly components to handle HTTP requests. It provides a complete HTTP
//! server implementation with support for:
//!
//! - Virtual hosting based on Host headers
//! - TLS/HTTPS connections
//! - Component isolation per request
//! - Graceful shutdown capabilities
//!
//! # Architecture
//!
//! The HTTP server plugin works by:
//! 1. Binding to a TCP socket and listening for connections
//! 2. Routing requests to components based on the Host header
//! 3. Creating isolated component instances for each request
//! 4. Managing the request/response lifecycle through WASI-HTTP
//! ```

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    path::Path,
    sync::Arc,
};

use crate::engine::ctx::Ctx;
use crate::engine::workload::ResolvedWorkload;
use crate::wit::WitInterface;
use anyhow::{Context, ensure};
use hyper::server::conn::http1;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use wasmtime::Store;
use wasmtime::component::InstancePre;
use wasmtime_wasi_http::{
    WasiHttpView,
    bindings::{ProxyPre, http::types::Scheme},
    body::HyperOutgoingBody,
    io::TokioIo,
};

use rustls::{ServerConfig, pki_types::CertificateDer};
use rustls_pemfile::{certs, private_key};
use tokio::sync::{RwLock, mpsc};
use tokio_rustls::TlsAcceptor;

/// Trait defining the routing behavior for HTTP requests
/// Allows for custom routing logic based on workload IDs and requests
/// Use this trait to implement custom routing strategies with the default HTTP Extension
#[async_trait::async_trait]
pub trait Router: Send + Sync + 'static {
    /// Register a workload that has been resolved
    /// and is guaranteed to be available for handling requests
    async fn on_workload_resolved(
        &self,
        resolved_handle: &ResolvedWorkload,
        component_id: &str,
    ) -> anyhow::Result<()>;

    /// Unregister a workload that is being stopped
    async fn on_workload_unbind(&self, workload_id: &str) -> anyhow::Result<()>;

    /// Determine if the outgoing request is allowed
    fn allow_outgoing_request(
        &self,
        workload_id: &str,
        request: &hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        config: &wasmtime_wasi_http::types::OutgoingRequestConfig,
    ) -> anyhow::Result<()>;

    /// Pick a workload ID based on the incoming request
    fn route_incoming_request(
        &self,
        req: &hyper::Request<hyper::body::Incoming>,
    ) -> anyhow::Result<String>;
}

/// Router that routes requests by 'Host' header, configured via WitInterface config
#[derive(Default)]
pub struct DynamicRouter {
    host_to_workload: tokio::sync::RwLock<HashMap<String, HashSet<String>>>,
    workload_to_host: tokio::sync::RwLock<HashMap<String, String>>,
}

/// Implementation of Router that maps Host headers to workload IDs
/// based on the 'host' config in the wasi:http/incoming-handler interface
#[async_trait::async_trait]
impl Router for DynamicRouter {
    async fn on_workload_resolved(
        &self,
        resolved_handle: &ResolvedWorkload,
        _component_id: &str,
    ) -> anyhow::Result<()> {
        let incoming_handler_interface = WitInterface::from("wasi:http/incoming-handler");
        let Some(http_iface) = resolved_handle
            .host_interfaces()
            .iter()
            .find(|iface| iface.contains(&incoming_handler_interface))
        else {
            anyhow::bail!("workload did not request wasi:http/incoming-handler interface");
        };

        let host_header = http_iface
            .config
            .get("host")
            .cloned()
            .context("No host header found")?;

        {
            let mut lock = self.workload_to_host.write().await;
            lock.insert(resolved_handle.id().to_string(), host_header.clone());
        }

        {
            let mut lock = self.host_to_workload.write().await;
            let entry = lock.entry(host_header.clone()).or_insert_with(HashSet::new);
            entry.insert(resolved_handle.id().to_string());
        }

        Ok(())
    }

    async fn on_workload_unbind(&self, workload_id: &str) -> anyhow::Result<()> {
        let mut lock = self.workload_to_host.write().await;
        if let Some(host_header) = lock.remove(workload_id) {
            let mut host_lock = self.host_to_workload.write().await;
            if let Some(workload_set) = host_lock.get_mut(&host_header) {
                workload_set.remove(workload_id);
                if workload_set.is_empty() {
                    host_lock.remove(&host_header);
                }
            }
        }
        Ok(())
    }

    fn allow_outgoing_request(
        &self,
        _workload_id: &str,
        _request: &hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        _config: &wasmtime_wasi_http::types::OutgoingRequestConfig,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Pick a workload ID based on the incoming request
    fn route_incoming_request(
        &self,
        req: &hyper::Request<hyper::body::Incoming>,
    ) -> anyhow::Result<String> {
        tokio::task::block_in_place(move || {
            let lock = self.host_to_workload.try_read()?;
            let workload_host = req
                .headers()
                .get(hyper::header::HOST)
                .and_then(|h| h.to_str().ok())
                .context("no Host header in request")?;
            let Some(workload_set) = lock.get(workload_host) else {
                anyhow::bail!("no workload bound to host header: {}", workload_host);
            };

            let workload_id = workload_set
                .iter()
                .next()
                .context("no workload IDs found for host header")?;

            Ok(workload_id.clone())
        })
    }
}

/// Development router that routes all requests to the last resolved workload
#[derive(Default)]
pub struct DevRouter {
    last_workload_id: tokio::sync::Mutex<Option<String>>,
}

#[async_trait::async_trait]
impl Router for DevRouter {
    async fn on_workload_resolved(
        &self,
        resolved_handle: &ResolvedWorkload,
        _component_id: &str,
    ) -> anyhow::Result<()> {
        let mut lock = self.last_workload_id.lock().await;
        lock.replace(resolved_handle.id().to_string());
        Ok(())
    }

    async fn on_workload_unbind(&self, workload_id: &str) -> anyhow::Result<()> {
        let mut lock = self.last_workload_id.lock().await;
        if let Some(current_id) = &*lock
            && current_id == workload_id
        {
            let _ = lock.take();
        }
        Ok(())
    }

    fn allow_outgoing_request(
        &self,
        _workload_id: &str,
        _request: &hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        _config: &wasmtime_wasi_http::types::OutgoingRequestConfig,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Pick a workload ID based on the incoming request
    fn route_incoming_request(
        &self,
        _req: &hyper::Request<hyper::body::Incoming>,
    ) -> anyhow::Result<String> {
        let lock = self.last_workload_id.try_lock()?;
        match &*lock {
            Some(id) => Ok(id.clone()),
            None => anyhow::bail!("no workload available to route request"),
        }
    }
}

/// Trait defining the behavior of a Host HTTP Extension
/// Allows for custom handling of incoming and outgoing HTTP requests
/// Use this trait to implement custom HTTP server transport
#[async_trait::async_trait]
pub trait HostHandler: Send + Sync + 'static {
    async fn start(&self) -> anyhow::Result<()>;
    async fn stop(&self) -> anyhow::Result<()>;

    async fn on_workload_resolved(
        &self,
        resolved_handle: &ResolvedWorkload,
        component_id: &str,
    ) -> anyhow::Result<()>;
    async fn on_workload_unbind(&self, workload_id: &str) -> anyhow::Result<()>;

    /// Refresh the cached InstancePre for a workload after component updates.
    /// This should be called after components are re-linked to ensure the HTTP
    /// handler uses the updated component chain.
    async fn refresh_workload_cache(
        &self,
        resolved_handle: &ResolvedWorkload,
    ) -> anyhow::Result<()>;

    fn outgoing_request(
        &self,
        workload_id: &str,
        request: hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        config: wasmtime_wasi_http::types::OutgoingRequestConfig,
    ) -> wasmtime_wasi_http::HttpResult<wasmtime_wasi_http::types::HostFutureIncomingResponse>;
}

impl std::fmt::Debug for dyn HostHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostHandler").finish()
    }
}

#[derive(Default)]
pub struct NullServer {}

#[async_trait::async_trait]
impl HostHandler for NullServer {
    async fn start(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn on_workload_resolved(
        &self,
        _resolved_handle: &ResolvedWorkload,
        _component_id: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn on_workload_unbind(&self, _workload_id: &str) -> anyhow::Result<()> {
        Ok(())
    }

    async fn refresh_workload_cache(
        &self,
        _resolved_handle: &ResolvedWorkload,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn outgoing_request(
        &self,
        _workload_id: &str,
        _request: hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        _config: wasmtime_wasi_http::types::OutgoingRequestConfig,
    ) -> wasmtime_wasi_http::HttpResult<wasmtime_wasi_http::types::HostFutureIncomingResponse> {
        Err(wasmtime_wasi_http::HttpError::trap(anyhow::anyhow!(
            "http client not available"
        )))
    }
}

/// A map from host header to resolved workload handles and their associated component id
pub type WorkloadHandles =
    Arc<RwLock<HashMap<String, (ResolvedWorkload, InstancePre<Ctx>, String)>>>;

/// HTTP server plugin that handles incoming HTTP requests for WebAssembly components.
///
/// This plugin implements the `wasi:http/incoming-handler` interface and routes
/// HTTP requests to appropriate WebAssembly components based on virtual hosting.
/// It supports both HTTP and HTTPS connections with optional mutual TLS.
pub struct HttpServer<T: Router> {
    router: Arc<T>,
    addr: SocketAddr,
    workload_handles: WorkloadHandles,
    shutdown_tx: Arc<RwLock<Option<mpsc::Sender<()>>>>,
    tls_acceptor: Option<TlsAcceptor>,
}

impl<T: Router> std::fmt::Debug for HttpServer<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpServer")
            .field("addr", &self.addr)
            .finish()
    }
}

impl<T: Router> HttpServer<T> {
    /// Creates a new HTTP server listening on the specified address.
    ///
    /// # Arguments
    /// * `router` - The router implementation for handling requests
    /// * `addr` - The socket address to bind to
    ///
    /// # Returns
    /// A new `HttpServer` instance configured for HTTP connections.
    pub fn new(router: T, addr: SocketAddr) -> Self {
        Self {
            router: Arc::new(router),
            addr,
            workload_handles: Arc::default(),
            shutdown_tx: Arc::new(RwLock::new(None)),
            tls_acceptor: None,
        }
    }

    /// Creates a new HTTPS server with TLS support.
    ///
    /// # Arguments
    /// * `router` - The router implementation for handling requests
    /// * `addr` - The socket address to bind to
    /// * `cert_path` - Path to the TLS certificate file
    /// * `key_path` - Path to the private key file
    /// * `ca_path` - Optional path to CA certificate for mutual TLS
    ///
    /// # Returns
    /// A new `HttpServer` instance configured for HTTPS connections.
    ///
    /// # Errors
    /// Returns an error if the TLS configuration cannot be loaded.
    pub async fn new_with_tls(
        router: T,
        addr: SocketAddr,
        cert_path: &Path,
        key_path: &Path,
        ca_path: Option<&Path>,
    ) -> anyhow::Result<Self> {
        let tls_config = load_tls_config(cert_path, key_path, ca_path).await?;
        let tls_acceptor = TlsAcceptor::from(Arc::new(tls_config));

        Ok(Self {
            router: Arc::new(router),
            addr,
            workload_handles: Arc::default(),
            shutdown_tx: Arc::new(RwLock::new(None)),
            tls_acceptor: Some(tls_acceptor),
        })
    }
}

#[async_trait::async_trait]
impl<T: Router> HostHandler for HttpServer<T> {
    async fn start(&self) -> anyhow::Result<()> {
        let addr = self.addr;
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        let shutdown_tx_clone = self.shutdown_tx.clone();
        let workload_handles = self.workload_handles.clone();
        let tls_acceptor = self.tls_acceptor.clone();

        // Store the shutdown sender
        *shutdown_tx_clone.write().await = Some(shutdown_tx);

        let listener = TcpListener::bind(addr).await?;
        info!(addr = ?addr, "HTTP server listening");
        // Start the HTTP server, any incoming requests call Host::handle and then it's routed
        // to the workload based on host header.
        let handler = self.router.clone();
        tokio::spawn(async move {
            if let Err(e) = run_http_server(
                listener,
                handler,
                workload_handles,
                &mut shutdown_rx,
                tls_acceptor,
            )
            .await
            {
                error!(err = ?e, addr = ?addr, "HTTP server error");
            }
        });

        let protocol = if self.tls_acceptor.is_some() {
            "HTTPS"
        } else {
            "HTTP"
        };
        debug!(addr = ?addr, protocol = protocol, "HTTP server starting");
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        info!(addr = ?self.addr, "HTTP server stopping");
        let mut shutdown_guard = self.shutdown_tx.write().await;
        if let Some(tx) = shutdown_guard.take() {
            let _ = tx.send(()).await;
        }
        Ok(())
    }

    async fn on_workload_resolved(
        &self,
        resolved_handle: &ResolvedWorkload,
        component_id: &str,
    ) -> anyhow::Result<()> {
        self.router
            .on_workload_resolved(resolved_handle, component_id)
            .await?;
        let instance_pre = resolved_handle.instantiate_pre(component_id).await?;

        self.workload_handles.write().await.insert(
            resolved_handle.id().to_string(),
            (
                resolved_handle.clone(),
                instance_pre,
                component_id.to_string(),
            ),
        );

        Ok(())
    }

    async fn on_workload_unbind(&self, workload_id: &str) -> anyhow::Result<()> {
        self.router.on_workload_unbind(workload_id).await?;

        self.workload_handles.write().await.remove(workload_id);

        Ok(())
    }

    async fn refresh_workload_cache(
        &self,
        resolved_handle: &ResolvedWorkload,
    ) -> anyhow::Result<()> {
        let workload_id = resolved_handle.id().to_string();
        let mut handles = self.workload_handles.write().await;

        // Only refresh if this workload is already registered
        if let Some((_, _, component_id)) = handles.get(&workload_id) {
            let component_id = component_id.clone();
            debug!(
                workload_id = %workload_id,
                component_id = %component_id,
                "refreshing HTTP handler cache after component update"
            );

            // Get a fresh InstancePre from the updated workload
            let instance_pre = resolved_handle.instantiate_pre(&component_id).await?;

            handles.insert(
                workload_id,
                (resolved_handle.clone(), instance_pre, component_id),
            );
        }

        Ok(())
    }

    fn outgoing_request(
        &self,
        workload_id: &str,
        request: hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        config: wasmtime_wasi_http::types::OutgoingRequestConfig,
    ) -> wasmtime_wasi_http::HttpResult<wasmtime_wasi_http::types::HostFutureIncomingResponse> {
        self.router
            .allow_outgoing_request(workload_id, &request, &config)
            .map_err(|e| {
                wasmtime_wasi_http::HttpError::trap(anyhow::anyhow!("request not allowed: {}", e))
            })?;

        // NOTE(lxf): Bring wasi-http code if needed
        // Separate HTTP / GRPC handling
        Ok(wasmtime_wasi_http::types::default_send_request(
            request, config,
        ))
    }
}
/// HTTP server implementation that routes to workload components
async fn run_http_server<T: Router>(
    listener: TcpListener,
    handler: Arc<T>,
    workload_handles: WorkloadHandles,
    shutdown_rx: &mut mpsc::Receiver<()>,
    tls_acceptor: Option<TlsAcceptor>,
) -> anyhow::Result<()> {
    loop {
        tokio::select! {
            // Handle shutdown signal
            _ = shutdown_rx.recv() => {
                info!("HTTP server received shutdown signal");
                break;
            }
            // Accept new connections
            result = listener.accept() => {
                match result {
                    Ok((client, client_addr)) => {
                        debug!(addr = ?client_addr, "new HTTP client connection");

                        let handles_clone = workload_handles.clone();
                        let tls_acceptor_clone = tls_acceptor.clone();
                        let handler_clone = handler.clone();
                        tokio::spawn(async move {
                            let service = hyper::service::service_fn(move |req| {
                                let handles = handles_clone.clone();
                                let handler = handler_clone.clone();
                                async move {
                                    handle_http_request(handler, req, handles).await
                                }
                            });

                            let result = if let Some(acceptor) = tls_acceptor_clone {
                                // Handle HTTPS connection
                                match acceptor.accept(client).await {
                                    Ok(tls_stream) => {
                                        http1::Builder::new()
                                            .keep_alive(true)
                                            .serve_connection(TokioIo::new(tls_stream), service)
                                            .await
                                    }
                                    Err(e) => {
                                        error!(addr = ?client_addr, err = ?e, "TLS handshake failed");
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
                                error!(addr = ?client_addr, err = ?e, "error serving HTTP client");
                            }
                        });
                    }
                    Err(e) => {
                        error!(err = ?e, "failed to accept HTTP connection");
                    }
                }
            }
        }
    }

    Ok(())
}

/// Handle individual HTTP requests by looking up workload and invoking component
async fn handle_http_request<T: Router>(
    handler: Arc<T>,
    req: hyper::Request<hyper::body::Incoming>,
    workload_handles: WorkloadHandles,
) -> Result<hyper::Response<HyperOutgoingBody>, hyper::Error> {
    let method = req.method().clone();
    let uri = req.uri().clone();

    let Ok(workload_id) = handler.route_incoming_request(&req) else {
        return Ok(hyper::Response::builder()
            .status(400)
            .body(HyperOutgoingBody::default())
            .expect("failed to build 400 response"));
    };

    debug!(
        method = %method,
        uri = %uri,
        host = %workload_id,
        "HTTP request received"
    );

    // NOTE(lxf): Separate HTTP / GRPC handling

    // Look up workload handle for this host, with wildcard fallback
    let workload_handle = {
        let handles = workload_handles.read().await;
        debug!(host = %workload_id, "looking up workload handle for host header");
        handles.get(&workload_id).cloned()
    };

    let response = match workload_handle {
        Some((handle, instance_pre, component_id)) => {
            match invoke_component_handler(handle, instance_pre, &component_id, req).await {
                Ok(resp) => resp,
                Err(e) => {
                    error!(err = ?e, host = %workload_id, "failed to invoke component");
                    hyper::Response::builder()
                        .status(500)
                        .body(HyperOutgoingBody::default())
                        // TODO: Add in the actual error message in the response body
                        // .body(HyperOutgoingBody::new(e.to_string()))
                        .expect("failed to build 500 response")
                }
            }
        }
        None => {
            warn!(host = %workload_id, "No workload bound to host header or wildcard '*'");
            hyper::Response::builder()
                .status(404)
                .body(HyperOutgoingBody::default())
                .expect("failed to build 404 response")
        }
    };

    Ok(response)
}

/// Invoke the component handler for the given workload
async fn invoke_component_handler(
    workload_handle: ResolvedWorkload,
    instance_pre: InstancePre<Ctx>,
    component_id: &str,
    req: hyper::Request<hyper::body::Incoming>,
) -> anyhow::Result<hyper::Response<HyperOutgoingBody>> {
    let components_arc = workload_handle.components();

    // Retry loop for reconciling/starting state
    // Re-read the component from the map on each iteration to get the latest state,
    // since the component object may be replaced during updates.
    let max_wait = std::time::Duration::from_secs(30);
    let poll_interval = std::time::Duration::from_millis(100);
    let start = std::time::Instant::now();

    loop {
        let state = {
            let components = components_arc.read().await;
            if let Some(comp) = components.get(component_id) {
                comp.get_state().await
            } else {
                // Component not found - might have been removed
                anyhow::bail!("component '{}' not found in workload", component_id);
            }
        };

        match state {
            crate::types::ComponentState::Running => break,
            crate::types::ComponentState::Reconciling => {
                if start.elapsed() > max_wait {
                    anyhow::bail!(
                        "component '{}' still reconciling after {:?}, request timed out",
                        component_id,
                        max_wait
                    );
                }
                tokio::time::sleep(poll_interval).await;
                continue;
            }
            crate::types::ComponentState::Stopped
            | crate::types::ComponentState::Stopping
            | crate::types::ComponentState::Error => {
                anyhow::bail!(
                    "component '{}' is not available (state: {:?})",
                    component_id,
                    state
                );
            }
            crate::types::ComponentState::Starting => {
                // Allow starting components - they may become ready
                if start.elapsed() > max_wait {
                    anyhow::bail!(
                        "component '{}' still starting after {:?}, request timed out",
                        component_id,
                        max_wait
                    );
                }
                tokio::time::sleep(poll_interval).await;
                continue;
            }
        }
    }

    // Get the current component (may have been replaced during update) and track in-flight request
    let component = {
        let components = components_arc.read().await;
        components.get(component_id).cloned()
    };

    if let Some(ref comp) = component {
        comp.begin_invocation();
    }

    // Create a new store for this request with plugin contexts
    let store = workload_handle.new_store(component_id).await?;

    // Execute the request and ensure we decrement the counter on completion
    let result = handle_component_request(store, instance_pre, req).await;

    // Decrement in-flight counter
    if let Some(ref comp) = component {
        comp.end_invocation();
    }

    result
}

/// Handle a component request using WASI HTTP (copied from wash/crates/src/cli/dev.rs)
pub async fn handle_component_request(
    mut store: Store<Ctx>,
    pre: InstancePre<Ctx>,
    req: hyper::Request<hyper::body::Incoming>,
) -> anyhow::Result<hyper::Response<HyperOutgoingBody>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let scheme = match req.uri().scheme() {
        Some(scheme) if scheme == &hyper::http::uri::Scheme::HTTP => Scheme::Http,
        Some(scheme) if scheme == &hyper::http::uri::Scheme::HTTPS => Scheme::Https,
        Some(scheme) => Scheme::Other(scheme.as_str().to_string()),
        // Fallback to HTTP if no scheme is present
        None => Scheme::Http,
    };
    let req = store.data_mut().new_incoming_request(scheme, req)?;
    let out = store.data_mut().new_response_outparam(sender)?;
    let pre = ProxyPre::new(pre).context("failed to instantiate proxy pre")?;

    // Run the http request itself in a separate task so the task can
    // optionally continue to execute beyond after the initial
    // headers/response code are sent.
    let task: JoinHandle<anyhow::Result<()>> = tokio::task::spawn(async move {
        // Run the http request itself by instantiating and calling the component
        let proxy = pre.instantiate_async(&mut store).await?;

        proxy
            .wasi_http_incoming_handler()
            .call_handle(&mut store, req, out)
            .await?;

        Ok(())
    });

    match receiver.await {
        // If the client calls `response-outparam::set` then one of these
        // methods will be called.
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(e)) => Err(e.into()),

        // Otherwise the `sender` will get dropped along with the `Store`
        // meaning that the oneshot will get disconnected
        Err(e) => {
            if let Err(task_error) = task.await {
                error!(err = ?task_error, "error receiving http response");
                Err(anyhow::anyhow!(
                    "error receiving http response: {task_error}"
                ))
            } else {
                error!(err = ?e, "error receiving http response");
                Err(anyhow::anyhow!(
                    "oneshot channel closed but no response was sent"
                ))
            }
        }
    }
}

/// Load TLS configuration from certificate and key files
/// Extracted from wash dev command for reuse in HTTP server plugin
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
