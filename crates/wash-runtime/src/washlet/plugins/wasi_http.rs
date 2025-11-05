//! # WASI HTTP Plugin
//!
//! This module implements an http-server capable of routing requests based on
//! the `Host` HTTP header to different wasmcloud workloads that
//! implement the `wasi:http/incoming-handler` interface.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;

const PLUGIN_HTTP_ID: &str = "wasi-http";

use crate::engine::ctx::Ctx;
use crate::engine::workload::{ResolvedWorkload, WorkloadComponent};
use crate::plugin::HostPlugin;
use crate::wit::WitWorld;
use anyhow::{Context, bail};
use hyper::server::conn::http1;
use opentelemetry::{Key, KeyValue};
use tokio::net::TcpListener;
use tokio::sync::{RwLock, mpsc};
use tokio_rustls::TlsAcceptor;
use tracing::{Instrument, debug, error, info, instrument, warn};
use wasmtime::component::InstancePre;
use wasmtime::{AsContextMut, StoreContextMut};
use wasmtime_wasi_http::WasiHttpView;
use wasmtime_wasi_http::bindings::ProxyPre;
use wasmtime_wasi_http::bindings::http::types::Scheme;
use wasmtime_wasi_http::body::HyperOutgoingBody;
use wasmtime_wasi_http::io::TokioIo;

use crate::washlet::host_invocation_span;
#[derive(Clone, Debug)]
struct HttpWorkloadConfig {
    workload_id_header: String,
}

type WorkloadHandles = Arc<RwLock<HashMap<String, (ResolvedWorkload, InstancePre<Ctx>, String)>>>;

pub struct HttpServer {
    addr: SocketAddr,
    /// Map from workload ID to resolved workload handles
    workload_handles: WorkloadHandles,
    /// Map from workload ID to HTTP-specific config
    workload_configs: Arc<RwLock<HashMap<String, HttpWorkloadConfig>>>,
    shutdown_tx: Arc<RwLock<Option<mpsc::Sender<()>>>>,
    metrics: Arc<HttpServerMetrics>,
}

struct HttpServerMetrics {
    requests_total: opentelemetry::metrics::Counter<u64>,
    request_duration: opentelemetry::metrics::Histogram<f64>,
}

impl HttpServerMetrics {
    fn new(meter: &opentelemetry::metrics::Meter) -> Self {
        let requests_total = meter
            .u64_counter("http_requests_total")
            .with_description("Total number of HTTP requests received")
            .build();
        let request_duration = meter
            .f64_histogram("http_request_duration_seconds")
            .with_description("Duration of HTTP requests in seconds")
            .build();
        Self {
            requests_total,
            request_duration,
        }
    }

    fn record_request(&self, method: impl AsRef<str>, status: u16, duration_secs: f64) {
        let attributes = [
            KeyValue::new(Key::new("http.status_code"), status as i64),
            KeyValue::new(Key::new("http.method"), method.as_ref().to_string()),
        ];
        self.requests_total.add(1, &attributes);
        self.request_duration.record(duration_secs, &attributes);
    }
}

impl HttpServer {
    pub fn new(addr: SocketAddr) -> Self {
        let meter = opentelemetry::global::meter("wasi-http");
        Self {
            addr,
            workload_handles: Arc::default(),
            workload_configs: Arc::default(),
            shutdown_tx: Arc::new(RwLock::new(None)),
            metrics: Arc::new(HttpServerMetrics::new(&meter)),
        }
    }
}

#[async_trait::async_trait]
impl HostPlugin for HttpServer {
    fn id(&self) -> &'static str {
        PLUGIN_HTTP_ID
    }
    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([crate::wit::WitInterface::from(
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
        let metrics = self.metrics.clone();

        // Store the shutdown sender
        *shutdown_tx_clone.write().await = Some(shutdown_tx);

        // Start the HTTP server, any incoming requests call Host::handle and then it's
        // routed to the workload based on host header.
        tokio::spawn(async move {
            if let Err(e) =
                run_http_server(addr, workload_handles, metrics, &mut shutdown_rx, None).await
            {
                error!(err = ?e, addr = ?addr, "HTTP server error");
            }
        });

        debug!(addr = ?addr,  "HTTP server starting");
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

        // Warnings for extra specified interfaces
        if interfaces.len() > 1 {
            warn!(
                interfaces = ?interfaces,
                "ignoring non-wasi:http/incoming-handler interfaces",
            );
        } else if http_iface.interfaces.len() > 1 {
            warn!(
                interfaces = ?http_iface.interfaces,
                "ignoring non-incoming-handler interfaces",
            );
        }

        let workload_id_header = http_iface
            .config
            .get("host")
            .cloned()
            .context("No host header found")?;
        let id = component.id();

        debug!(header = %workload_id_header, workload_id = id, "binding HTTP config for workload");

        // Store config by workload ID for later retrieval
        let config = HttpWorkloadConfig { workload_id_header };
        self.workload_configs
            .write()
            .await
            .insert(id.to_string(), config);

        // NOTE: There is no `add_to_linker` call here because it's already added when
        // initializing the Ctx, as long as the `http` feature is enabled. This
        // is totally possible to do here, but it would mean re-implementing
        // wasmtime_wasi_http.

        Ok(())
    }

    async fn on_workload_resolved(
        &self,
        resolved_handle: &ResolvedWorkload,
        id: &str,
    ) -> anyhow::Result<()> {
        // Retrieve config using the same ID from bind_workload
        let config = self
            .workload_configs
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No HTTP config found for workload {}", id))?;

        debug!(host = %config.workload_id_header, workload_id = id, "storing resolved workload handle");

        // Get the pre-instantiated component
        let instance_pre = resolved_handle.instantiate_pre(id).await?;

        // Store the resolved handle with the configured host header
        self.workload_handles.write().await.insert(
            config.workload_id_header,
            (resolved_handle.clone(), instance_pre, id.to_string()),
        );

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
    addr: SocketAddr,
    workload_handles: WorkloadHandles,
    metrics: Arc<HttpServerMetrics>,
    shutdown_rx: &mut mpsc::Receiver<()>,
    tls_acceptor: Option<TlsAcceptor>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    debug!(addr = ?addr, "HTTP server listening");

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
                        let metrics_clone = metrics.clone();
                        tokio::spawn(async move {
                            let service = hyper::service::service_fn(move |req| {
                                let handles = handles_clone.clone();
                                let metrics = metrics_clone.clone();
                                async move {
                                    let request_start_time = std::time::Instant::now();
                                    let method  = req.method().clone();

                                    match handle_http_request(req, handles).await {
                                        Ok(response) => {
                                            let request_duration = request_start_time.elapsed();
                                            metrics.record_request(
                                                method.as_str(),
                                                response.status().as_u16(),
                                                request_duration.as_millis() as f64,
                                            );
                                            Ok(response)
                                        },
                                        Err(e) => Err(e)
                                    }

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

/// Handle individual HTTP requests by looking up workload and invoking
/// component
#[instrument(name = "WasiHTTP::handle_http_request", skip(req, workload_handles))]
async fn handle_http_request(
    req: hyper::Request<hyper::body::Incoming>,
    workload_handles: WorkloadHandles,
) -> Result<hyper::Response<HyperOutgoingBody>, hyper::http::Error> {
    let method = req.method().clone();
    let uri = req.uri().clone();

    let workload_id_header = match req.headers().get("host").and_then(|h| h.to_str().ok()) {
        Some(id) => id.to_string(),
        None => {
            warn!("No host header");
            return hyper::Response::builder()
                .status(400)
                .body(HyperOutgoingBody::default());
        }
    };

    debug!(
        method = %method,
        uri = %uri,
        workload_id = %workload_id_header,
        "HTTP request received"
    );

    // Look up workload handle for this header
    let workload_handle = {
        let handles = workload_handles.read().await;
        debug!(workload_id = %workload_id_header, "looking up workload handle");

        handles.get(&workload_id_header).cloned()
    };

    match workload_handle {
        Some((handle, instance_pre, id)) => {
            let span = host_invocation_span(
                handle.namespace(),
                handle.name(),
                &id,
                PLUGIN_HTTP_ID,
                "handle_http_request",
            );

            match invoke_component_handler(handle, instance_pre, &id, req)
                .instrument(span)
                .await
            {
                Ok(resp) => Ok(resp),
                Err(e) => {
                    error!(err = ?e, workload_id = %workload_id_header, "Failed to invoke component");
                    hyper::Response::builder()
                        .status(502)
                        .body(HyperOutgoingBody::default())
                }
            }
        }
        None => {
            warn!(workload_id = %workload_id_header, "No workload found");
            hyper::Response::builder()
                .status(400)
                .body(HyperOutgoingBody::default())
        }
    }
}

/// Invoke the component handler for the given workload
async fn invoke_component_handler(
    workload_handle: ResolvedWorkload,
    instance_pre: InstancePre<Ctx>,
    id: &str,
    req: hyper::Request<hyper::body::Incoming>,
) -> anyhow::Result<hyper::Response<HyperOutgoingBody>> {
    // Create a new store for this request with plugin contexts
    let mut store = workload_handle.new_store(id).await?;

    // Use the same implementation as dev.rs
    handle_component_request(store.as_context_mut(), instance_pre, req).await
}

/// Handle a component request using WASI HTTP (copied from
/// wash/crates/src/cli/dev.rs)
pub async fn handle_component_request<'a>(
    mut store: StoreContextMut<'a, Ctx>,
    pre: InstancePre<Ctx>,
    req: hyper::Request<hyper::body::Incoming>,
) -> anyhow::Result<hyper::Response<HyperOutgoingBody>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let req = store.data_mut().new_incoming_request(Scheme::Http, req)?;
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
