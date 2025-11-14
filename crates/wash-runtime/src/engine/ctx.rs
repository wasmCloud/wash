//! Component execution context for wasmtime stores.
//!
//! This module provides the [`Ctx`] type which serves as the store context
//! for wasmtime when executing WebAssembly components. It integrates WASI
//! interfaces, HTTP capabilities, and plugin access into a unified context.

use std::{any::Any, collections::HashMap, sync::Arc};

use wasmtime::component::ResourceTable;
use wasmtime_wasi::{IoView, WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpView};

use crate::plugin::HostPlugin;

/// The context for a component store and linker, providing access to implementations of:
/// - wasi@0.2 interfaces
/// - wasi:http@0.2 interfaces
pub struct Ctx {
    /// Unique identifier for this component context. This is a [uuid::Uuid::new_v4] string.
    pub id: String,
    /// The unique identifier for the workload component this instance belongs to
    pub component_id: Arc<str>,
    /// The unique identifier for the workload this component belongs to
    pub workload_id: Arc<str>,
    /// The resource table used to manage resources in the Wasmtime store.
    pub table: wasmtime::component::ResourceTable,
    /// The WASI context used to provide WASI functionality to the components using this context.
    pub ctx: WasiCtx,
    /// The HTTP context used to provide HTTP functionality to the component.
    pub http: WasiHttpCtx,
    /// Plugin instances stored by string ID for access during component execution.
    /// These all implement the [`HostPlugin`] trait, but they are cast as `Arc<dyn Any + Send + Sync>`
    /// to support downcasting to the specific plugin type in [`Ctx::get_plugin`]
    plugins: HashMap<&'static str, Arc<dyn Any + Send + Sync>>,
}

impl Ctx {
    /// Get a plugin by its string ID and downcast to the expected type
    pub fn get_plugin<T: HostPlugin + 'static>(&self, plugin_id: &str) -> Option<Arc<T>> {
        self.plugins.get(plugin_id)?.clone().downcast().ok()
    }

    /// Create a new [`CtxBuilder`] to construct a [`Ctx`]
    pub fn builder(
        workload_id: impl Into<Arc<str>>,
        component_id: impl Into<Arc<str>>,
    ) -> CtxBuilder {
        CtxBuilder::new(workload_id, component_id)
    }
}

impl std::fmt::Debug for Ctx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx")
            .field("id", &self.id)
            .field("workload_id", &self.workload_id.as_ref())
            .field("table", &self.table)
            .finish()
    }
}

impl IoView for Ctx {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}
// TODO(#103): Do some cleverness to pull up the WasiCtx based on what component is actively executing
impl WasiView for Ctx {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.ctx
    }
}

// Implement WasiHttpView for wasi:http@0.2
impl WasiHttpView for Ctx {
    fn ctx(&mut self) -> &mut WasiHttpCtx {
        &mut self.http
    }

    fn send_request(
        &mut self,
        request: hyper::Request<wasmtime_wasi_http::body::HyperOutgoingBody>,
        config: wasmtime_wasi_http::types::OutgoingRequestConfig,
    ) -> wasmtime_wasi_http::HttpResult<wasmtime_wasi_http::types::HostFutureIncomingResponse> {
        use http_body_util::BodyExt;
        use wasmtime_wasi_http::types::{HostFutureIncomingResponse, IncomingResponse};

        // Detect if this is a gRPC request by checking Content-Type header
        let is_grpc = request
            .headers()
            .get(hyper::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.starts_with("application/grpc"))
            .unwrap_or(false);

        // Route to appropriate client plugin
        if is_grpc {
            // Use gRPC client for gRPC requests
            if let Some(plugin) =
                self.get_plugin::<crate::plugin::wasi_grpc_client::GrpcClient>("grpc-client")
            {
                tracing::debug!(
                    method = %request.method(),
                    uri = %request.uri(),
                    use_tls = config.use_tls,
                    "sending gRPC request with HTTP/2 client"
                );

                let clients = plugin.clients.clone();
                let between_bytes_timeout = config.between_bytes_timeout;

                Ok(HostFutureIncomingResponse::Pending(
                    wasmtime_wasi::runtime::spawn(async move {
                        match clients.request(request).await {
                            Ok(resp) => Ok(Ok(IncomingResponse {
                                resp: resp.map(|body| body.map_err(|e| {
                                    tracing::warn!(?e, "gRPC body error");
                                    wasmtime_wasi_http::bindings::http::types::ErrorCode::HttpProtocolError
                                }).boxed()),
                                worker: None,
                                between_bytes_timeout,
                            })),
                            Err(e) => {
                                tracing::warn!(?e, "gRPC request failed");
                                Ok(Err(wasmtime_wasi_http::bindings::http::types::ErrorCode::HttpProtocolError))
                            }
                        }
                    }),
                ))
            } else {
                tracing::error!("gRPC client plugin not available - cannot send gRPC request");
                Ok(HostFutureIncomingResponse::Ready(Ok(Err(
                    wasmtime_wasi_http::bindings::http::types::ErrorCode::InternalError(Some(
                        "gRPC client plugin not configured".to_string(),
                    )),
                ))))
            }
        } else {
            // Use HTTP client for regular HTTP requests
            if let Some(plugin) =
                self.get_plugin::<crate::plugin::wasi_http_client::HttpClient>("http-client")
            {
                tracing::debug!(
                    method = %request.method(),
                    uri = %request.uri(),
                    use_tls = config.use_tls,
                    "sending HTTP request with custom client"
                );

                let clients = plugin.clients.clone();
                let between_bytes_timeout = config.between_bytes_timeout;

                Ok(HostFutureIncomingResponse::Pending(
                    wasmtime_wasi::runtime::spawn(async move {
                        match clients.request(request).await {
                            Ok(resp) => Ok(Ok(IncomingResponse {
                                resp: resp.map(|body| body.map_err(|e| {
                                    tracing::warn!(?e, "HTTP body error");
                                    wasmtime_wasi_http::bindings::http::types::ErrorCode::HttpProtocolError
                                }).boxed()),
                                worker: None,
                                between_bytes_timeout,
                            })),
                            Err(e) => {
                                tracing::warn!(?e, "HTTP request failed");
                                Ok(Err(wasmtime_wasi_http::bindings::http::types::ErrorCode::HttpProtocolError))
                            }
                        }
                    }),
                ))
            } else {
                tracing::error!("HTTP client plugin not available - cannot send HTTP request");
                Ok(HostFutureIncomingResponse::Ready(Ok(Err(
                    wasmtime_wasi_http::bindings::http::types::ErrorCode::InternalError(Some(
                        "HTTP client plugin not configured".to_string(),
                    )),
                ))))
            }
        }
    }
}

/// Helper struct to build a [`Ctx`] with a builder pattern
pub struct CtxBuilder {
    id: String,
    workload_id: Arc<str>,
    component_id: Arc<str>,
    ctx: Option<WasiCtx>,
    plugins: HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>,
}

impl CtxBuilder {
    pub fn new(workload_id: impl Into<Arc<str>>, component_id: impl Into<Arc<str>>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            component_id: component_id.into(),
            workload_id: workload_id.into(),
            ctx: None,
            plugins: HashMap::new(),
        }
    }

    pub fn with_wasi_ctx(mut self, ctx: WasiCtx) -> Self {
        self.ctx = Some(ctx);
        self
    }

    pub fn with_plugins(
        mut self,
        plugins: HashMap<&'static str, Arc<dyn HostPlugin + Send + Sync>>,
    ) -> Self {
        self.plugins.extend(plugins);
        self
    }

    pub fn build(self) -> Ctx {
        let plugins = self
            .plugins
            .into_iter()
            .map(|(k, v)| (k, v as Arc<dyn Any + Send + Sync>))
            .collect();

        Ctx {
            id: self.id,
            ctx: self.ctx.unwrap_or_else(|| {
                WasiCtxBuilder::new()
                    .args(&["main.wasm"])
                    .inherit_stderr()
                    .build()
            }),
            workload_id: self.workload_id,
            component_id: self.component_id,
            http: WasiHttpCtx::new(),
            table: ResourceTable::new(),
            plugins,
        }
    }
}
