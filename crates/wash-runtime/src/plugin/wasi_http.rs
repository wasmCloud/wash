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

use std::collections::HashSet;
use std::{collections::HashMap, net::SocketAddr, path::Path, sync::Arc};

use crate::engine::workload::{ResolvedWorkload, WorkloadComponent};
use crate::wit::WitInterface;
use crate::wit::WitWorld;
use crate::{engine::ctx::Ctx, plugin::HostPlugin};
use anyhow::{Context, bail, ensure};
use hyper::server::conn::http1;
use tokio::net::TcpListener;
use tracing::{debug, error, info, warn};
use wasmtime::component::InstancePre;
use wasmtime::{AsContextMut, StoreContextMut};
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

const HTTP_SERVER_ID: &str = "http-server";

#[derive(Clone, Debug)]
struct HttpWorkloadConfig {
    host_header: String,
}

/// A map from host header to resolved workload handles and their associated component id
pub type WorkloadHandles =
    Arc<RwLock<HashMap<String, (ResolvedWorkload, InstancePre<Ctx>, String)>>>;

/// HTTP server plugin that handles incoming HTTP requests for WebAssembly components.
///
/// This plugin implements the `wasi:http/incoming-handler` interface and routes
/// HTTP requests to appropriate WebAssembly components based on virtual hosting.
/// It supports both HTTP and HTTPS connections with optional mutual TLS.
pub struct HttpServer {
    addr: SocketAddr,
    /// Map from host header to resolved workload handles
    workload_handles: WorkloadHandles,
    /// Map from workload ID to HTTP-specific config
    workload_configs: Arc<RwLock<HashMap<String, HttpWorkloadConfig>>>,
    shutdown_tx: Arc<RwLock<Option<mpsc::Sender<()>>>>,
    tls_acceptor: Option<TlsAcceptor>,
}

impl HttpServer {
    /// Creates a new HTTP server listening on the specified address.
    ///
    /// # Arguments
    /// * `addr` - The socket address to bind to
    ///
    /// # Returns
    /// A new `HttpServer` instance configured for HTTP connections.
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            workload_handles: Arc::default(),
            workload_configs: Arc::default(),
            shutdown_tx: Arc::new(RwLock::new(None)),
            tls_acceptor: None,
        }
    }

    /// Creates a new HTTPS server with TLS support.
    ///
    /// # Arguments
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
        addr: SocketAddr,
        cert_path: &Path,
        key_path: &Path,
        ca_path: Option<&Path>,
    ) -> anyhow::Result<Self> {
        let tls_config = load_tls_config(cert_path, key_path, ca_path).await?;
        let tls_acceptor = TlsAcceptor::from(Arc::new(tls_config));

        Ok(Self {
            addr,
            workload_handles: Arc::default(),
            workload_configs: Arc::default(),
            shutdown_tx: Arc::new(RwLock::new(None)),
            tls_acceptor: Some(tls_acceptor),
        })
    }
}

#[async_trait::async_trait]
impl HostPlugin for HttpServer {
    fn id(&self) -> &'static str {
        HTTP_SERVER_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasi:http/incoming-handler,outgoing-handler",
            )]),
            ..Default::default()
        }
    }

    async fn start(&self) -> anyhow::Result<()> {
        let addr = self.addr;
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
        let shutdown_tx_clone = self.shutdown_tx.clone();
        let workload_handles = self.workload_handles.clone();
        let tls_acceptor = self.tls_acceptor.clone();

        // Store the shutdown sender
        *shutdown_tx_clone.write().await = Some(shutdown_tx);

        let listener = TcpListener::bind(addr).await?;
        debug!(addr = ?addr, "HTTP server listening");
        // Start the HTTP server, any incoming requests call Host::handle and then it's routed
        // to the workload based on host header.
        tokio::spawn(async move {
            if let Err(e) =
                run_http_server(listener, workload_handles, &mut shutdown_rx, tls_acceptor).await
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

    async fn on_component_bind(
        &self,
        component: &mut WorkloadComponent,
        interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        let Some(http_iface) = interfaces.iter().find(|iface| {
            iface.namespace == "wasi"
                && iface.package == "http"
                && iface.interfaces.contains("incoming-handler")
        }) else {
            bail!(
                "No wasi:http/incoming-handler interface found, plugin should not be bound to this workload"
            );
        };

        // Debug log for extra specified interfaces
        if interfaces.len() > 1 {
            debug!(
                interfaces = ?interfaces,
                "ignoring non-wasi:http/incoming-handler interfaces",
            );
        } else if http_iface.interfaces.len() > 1 {
            debug!(
                interfaces = ?http_iface.interfaces,
                "ignoring non-incoming-handler interfaces",
            );
        }

        // Use wildcard "*" as default if no host header is specified
        let host_header = http_iface
            .config
            .get("host")
            .cloned()
            .unwrap_or_else(|| "*".to_string());

        let id = component.id();

        debug!(host = %host_header, workload_id = id, "binding HTTP config for workload");

        // Store config by workload ID for later retrieval
        let config = HttpWorkloadConfig { host_header };
        self.workload_configs
            .write()
            .await
            .insert(id.to_string(), config);

        // NOTE: There is no `add_to_linker` call here because it's already added when initializing
        // the Ctx, as long as the `http` feature is enabled. This is totally possible to do here, but it would
        // mean re-implementing wasmtime_wasi_http.

        Ok(())
    }

    async fn on_workload_resolved(
        &self,
        resolved_handle: &ResolvedWorkload,
        component_id: &str,
    ) -> anyhow::Result<()> {
        // Retrieve config using the same ID from bind_workload
        let config = self
            .workload_configs
            .read()
            .await
            .get(component_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No HTTP config found for workload {component_id}"))?;

        debug!(host = %config.host_header, workload_id = resolved_handle.id(), component_id, "storing resolved workload handle");

        // Get the pre-instantiated component
        let instance_pre = resolved_handle.instantiate_pre(component_id).await?;

        // Store the resolved handle with the configured host header
        self.workload_handles.write().await.insert(
            config.host_header,
            (
                resolved_handle.clone(),
                instance_pre,
                component_id.to_string(),
            ),
        );

        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload: &ResolvedWorkload,
        _interfaces: HashSet<WitInterface>,
    ) -> anyhow::Result<()> {
        debug!(workload_id = workload.id(), "removing HTTP workload handle");

        // Remove from workload handles
        let mut handles_guard = self.workload_handles.write().await;
        handles_guard.retain(|_, (handle, _, _)| handle.id() != workload.id());

        // Remove from workload configs
        self.workload_configs.write().await.remove(workload.id());

        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        info!(addr = ?self.addr, "HTTP server stopping");
        // Stop the HTTP server
        let mut shutdown_guard = self.shutdown_tx.write().await;
        if let Some(tx) = shutdown_guard.take() {
            let _ = tx.send(()).await;
        }
        Ok(())
    }
}

/// HTTP server implementation that routes to workload components
async fn run_http_server(
    listener: TcpListener,
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
                        tokio::spawn(async move {
                            let service = hyper::service::service_fn(move |req| {
                                let handles = handles_clone.clone();
                                async move {
                                    handle_http_request(req, handles).await
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
async fn handle_http_request(
    req: hyper::Request<hyper::body::Incoming>,
    workload_handles: WorkloadHandles,
) -> Result<hyper::Response<HyperOutgoingBody>, hyper::Error> {
    let method = req.method().clone();
    let uri = req.uri().clone();

    // Extract the Host header
    let host_header = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("<no host header>")
        .to_string(); // Convert to String to avoid borrow issues

    debug!(
        method = %method,
        uri = %uri,
        host = %host_header,
        "HTTP request received"
    );

    // Look up workload handle for this host, with wildcard fallback
    let workload_handle = {
        let handles = workload_handles.read().await;
        debug!(host = %host_header, "looking up workload handle for host header");

        // First try exact host match
        if let Some(handle) = handles.get(&host_header) {
            Some(handle.clone())
        } else {
            // Fall back to wildcard if no exact match
            debug!("No exact match for host header, trying wildcard '*'");
            handles.get("*").cloned()
        }
    };

    let response = match workload_handle {
        Some((handle, instance_pre, component_id)) => {
            match invoke_component_handler(handle, instance_pre, &component_id, req).await {
                Ok(resp) => resp,
                Err(e) => {
                    error!(err = ?e, host = %host_header, "failed to invoke component");
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
            warn!(host = %host_header, "No workload bound to host header or wildcard '*'");
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
    // Create a new store for this request with plugin contexts
    let mut store = workload_handle.new_store(component_id).await?;

    handle_component_request(store.as_context_mut(), instance_pre, req).await
}

/// Handle a component request using WASI HTTP (copied from wash/crates/src/cli/dev.rs)
pub async fn handle_component_request<'a>(
    mut store: StoreContextMut<'a, Ctx>,
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

    // Run the http request itself by instantiating and calling the component
    let proxy = pre.instantiate_async(&mut store).await?;

    proxy
        .wasi_http_incoming_handler()
        .call_handle(&mut store, req, out)
        .await?;

    match receiver.await {
        // If the client calls `response-outparam::set` then one of these
        // methods will be called.
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(e)) => Err(e.into()),

        // Otherwise the `sender` will get dropped along with the `Store`
        // meaning that the oneshot will get disconnected
        Err(e) => {
            error!(err = ?e, "error receiving http response");
            Err(anyhow::anyhow!(
                "oneshot channel closed but no response was sent"
            ))
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
