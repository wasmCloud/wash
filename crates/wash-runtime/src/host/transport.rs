//! Modular outgoing request handling for HTTP server

use std::collections::HashMap;
use std::sync::Arc;
use wasmtime_wasi_http::{
    HttpResult,
    body::HyperOutgoingBody,
    types::{HostFutureIncomingResponse, OutgoingRequestConfig},
};

/// Trait for handling outgoing requests with specific transport protocols
pub trait OutgoingHandler: Send + Sync + 'static {
    /// Check if this handler can handle the given request
    fn can_handle(&self, request: &hyper::Request<HyperOutgoingBody>) -> bool;

    /// Send the request using this handler's transport
    fn send_request(
        &self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse>;
}

/// Default HTTP/1.1 and HTTP/2 handler
#[derive(Clone, Default)]
pub struct DefaultHttpHandler;

impl OutgoingHandler for DefaultHttpHandler {
    fn can_handle(&self, _request: &hyper::Request<HyperOutgoingBody>) -> bool {
        true // Accepts everything
    }

    fn send_request(
        &self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        Ok(wasmtime_wasi_http::types::default_send_request(
            request, config,
        ))
    }
}

/// gRPC handler
#[cfg(feature = "grpc")]
pub struct GrpcHandler {
    _config: HashMap<String, String>,
}

#[cfg(feature = "grpc")]
impl GrpcHandler {
    pub fn new(config: HashMap<String, String>) -> anyhow::Result<Self> {
        crate::grpc::init_grpc(&config)?;
        Ok(Self { _config: config })
    }
}

#[cfg(feature = "grpc")]
impl OutgoingHandler for GrpcHandler {
    fn can_handle(&self, request: &hyper::Request<HyperOutgoingBody>) -> bool {
        request
            .headers()
            .get(hyper::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.starts_with("application/grpc"))
            .unwrap_or(false)
    }

    fn send_request(
        &self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        crate::grpc::send_request(request, config)
    }
}

/// Chains multiple outgoing handlers together
pub struct CompositeOutgoingHandler {
    handlers: Vec<Arc<dyn OutgoingHandler>>,
    default: Arc<dyn OutgoingHandler>,
}

impl CompositeOutgoingHandler {
    pub fn new() -> Self {
        Self {
            handlers: Vec::new(),
            default: Arc::new(DefaultHttpHandler),
        }
    }

    pub fn with_handler(mut self, handler: Arc<dyn OutgoingHandler>) -> Self {
        self.handlers.push(handler);
        self
    }

    #[cfg(feature = "grpc")]
    pub fn with_grpc(self, config: HashMap<String, String>) -> anyhow::Result<Self> {
        let grpc = GrpcHandler::new(config)?;
        Ok(self.with_handler(Arc::new(grpc)))
    }

    pub fn send_request(
        &self,
        request: hyper::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        for handler in &self.handlers {
            if handler.can_handle(&request) {
                tracing::debug!(
                    uri = %request.uri(),
                    "routing outgoing request to custom handler"
                );
                return handler.send_request(request, config);
            }
        }

        tracing::debug!(uri = %request.uri(), "routing to default HTTP handler");
        self.default.send_request(request, config)
    }
}

impl Default for CompositeOutgoingHandler {
    fn default() -> Self {
        Self::new()
    }
}
