//! gRPC client implementation for wash-runtime
//!
//! This module provides a gRPC client capable of handling HTTP/2 connections,
//! including support for TLS and ALPN for `h2`. It is designed to be used
//! with the [`CompositeOutgoingHandler`](super::CompositeOutgoingHandler) for
//! routing gRPC requests through HTTP/2.

use anyhow::{Context, Result};
use http_body_util::BodyExt;
use hyper::Uri;
use hyper::body::Incoming;
use hyper_util::{
    client::legacy::{Client, connect::Connection},
    rt::{TokioExecutor, TokioIo},
};
use once_cell::sync::OnceCell;
use rustls::pki_types::ServerName;
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc, task::Poll};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tower_service::Service;
use tracing::debug;
use wasmtime_wasi_http::{
    HttpResult,
    body::HyperOutgoingBody,
    types::{HostFutureIncomingResponse, IncomingResponse, OutgoingRequestConfig},
};

/// HTTP/2 protocol identifier for ALPN
const ALPN_H2: &[u8] = b"h2";
const GRPC_CONTENT_TYPE: &str = "application/grpc";

static GRPC_CLIENTS: OnceCell<GrpcClients> = OnceCell::new();

pub fn init_grpc(config: &HashMap<String, String>) -> anyhow::Result<()> {
    GRPC_CLIENTS.get_or_try_init(|| -> anyhow::Result<GrpcClients> {
        let enable_pooling = config
            .get("enable_pooling")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);

        let tls_config = configure_tls(config)?;
        Ok(GrpcClients::new(enable_pooling, tls_config))
    })?;
    Ok(())
}

/// Wrapper for TLS stream that implements Connection trait
pub struct RustlsStream(TlsStream<TcpStream>);

impl Connection for RustlsStream {
    fn connected(&self) -> hyper_util::client::legacy::connect::Connected {
        // Check ALPN negotiation result and signal to hyper
        let (_, session) = self.0.get_ref();
        if session.alpn_protocol() == Some(ALPN_H2) {
            hyper_util::client::legacy::connect::Connected::new().negotiated_h2()
        } else {
            hyper_util::client::legacy::connect::Connected::new()
        }
    }
}

impl tokio::io::AsyncRead for RustlsStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for RustlsStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.get_mut().0).poll_write(cx, buf)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.get_mut().0).poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.get_mut().0).poll_shutdown(cx)
    }
}

/// HTTP connector for plain TCP connections
#[derive(Clone)]
pub struct HttpConnector;

impl Service<Uri> for HttpConnector {
    type Response = TokioIo<TcpStream>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        Box::pin(async move {
            let authority = uri.authority().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing authority")
            })?;

            let host = authority.host();
            let port = authority.port_u16().unwrap_or(80);

            debug!(host, port, "connecting to HTTP endpoint");
            let stream = TcpStream::connect((host, port)).await?;
            Ok(TokioIo::new(stream))
        })
    }
}

/// HTTPS connector with TLS and ALPN negotiation
#[derive(Clone)]
pub struct HttpsConnector {
    tls_config: Arc<rustls::ClientConfig>,
}

impl HttpsConnector {
    pub fn new(tls_config: Arc<rustls::ClientConfig>) -> Self {
        Self { tls_config }
    }
}

impl Service<Uri> for HttpsConnector {
    type Response = TokioIo<RustlsStream>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        let tls_config = self.tls_config.clone();
        Box::pin(async move {
            let authority = uri.authority().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing authority")
            })?;

            let host = authority.host();
            let port = authority.port_u16().unwrap_or(443);

            debug!(host, port, "connecting to HTTPS endpoint");
            let stream = TcpStream::connect((host, port)).await?;

            let connector = tokio_rustls::TlsConnector::from(tls_config);
            let server_name = ServerName::try_from(host)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?
                .to_owned();

            let tls_stream = connector.connect(server_name, stream).await?;
            Ok(TokioIo::new(RustlsStream(tls_stream)))
        })
    }
}
/// gRPC-specific HTTP clients with automatic connection pooling
///
/// gRPC requires HTTP/2, so we only use HTTP/2 clients
#[derive(Clone)]
pub struct GrpcClients {
    /// HTTP/2 client for plain TCP (h2c - cleartext HTTP/2)
    h2c: Client<HttpConnector, HyperOutgoingBody>,
    /// HTTP/2 over TLS (standard gRPC)
    h2: Client<HttpsConnector, HyperOutgoingBody>,
}

impl GrpcClients {
    pub fn new(enable_pooling: bool, tls_config: Arc<rustls::ClientConfig>) -> Self {
        let mut h2c_builder = Client::builder(TokioExecutor::new());
        let mut h2_builder = Client::builder(TokioExecutor::new());

        if !enable_pooling {
            h2c_builder.pool_max_idle_per_host(0);
            h2_builder.pool_max_idle_per_host(0);
        }

        // Configure both clients to use HTTP/2 only (prior knowledge for h2c, ALPN for h2)
        // This is required for gRPC which mandates HTTP/2
        let h2c_builder = h2c_builder.http2_only(true);
        let h2_builder = h2_builder.http2_only(true);

        Self {
            h2c: h2c_builder.build(HttpConnector),
            h2: h2_builder.build(HttpsConnector::new(tls_config)),
        }
    }
    /// Send a gRPC request using the appropriate client
    ///
    /// This automatically adds gRPC-required headers and enforces HTTP/2
    pub async fn request(
        &self,
        mut request: hyper::Request<HyperOutgoingBody>,
    ) -> Result<hyper::Response<Incoming>, hyper_util::client::legacy::Error> {
        // Ensure gRPC content-type header
        if !request.headers().contains_key(hyper::header::CONTENT_TYPE) {
            request.headers_mut().insert(
                hyper::header::CONTENT_TYPE,
                hyper::header::HeaderValue::from_static(GRPC_CONTENT_TYPE),
            );
        }

        // gRPC requires HTTP/2
        let use_tls = request.uri().scheme() == Some(&hyper::http::uri::Scheme::HTTPS);

        if use_tls {
            tracing::debug!(uri = %request.uri(), "sending gRPC request over HTTP/2 (TLS)");
            self.h2.request(request).await
        } else {
            tracing::debug!(uri = %request.uri(), "sending gRPC request over HTTP/2 (h2c)");
            self.h2c.request(request).await
        }
    }
}

/// Configure TLS for gRPC (requires ALPN with h2)
fn configure_tls(config: &HashMap<String, String>) -> anyhow::Result<Arc<rustls::ClientConfig>> {
    // This is required for rustls 0.23+
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let mut ca = rustls::RootCertStore::empty();
    // Load Mozilla trusted root certificates
    if config
        .get("load_webpki_certs")
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(true)
    {
        ca.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        debug!("loaded webpki root certificate store for gRPC");
    }

    // Load root certificates from a file
    if let Some(file_path) = config.get("ssl_certs_file") {
        let f = std::fs::File::open(file_path).context("failed to open SSL certificate file")?;
        let mut reader = std::io::BufReader::new(f);
        let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
            rustls_pemfile::certs(&mut reader)
                .collect::<Result<Vec<_>, _>>()
                .context("failed to parse certificates")?;
        let (added, ignored) = ca.add_parsable_certificates(certs);
        debug!(
            added,
            ignored, file_path, "added additional root certificates for gRPC"
        );
    }

    // Configure ALPN for gRPC (HTTP/2 only)
    let mut tls_config = rustls::ClientConfig::builder()
        .with_root_certificates(ca)
        .with_no_client_auth();

    // gRPC requires HTTP/2
    tls_config.alpn_protocols = vec![ALPN_H2.to_vec()];

    Ok(Arc::new(tls_config))
}

pub fn send_request(
    request: hyper::Request<HyperOutgoingBody>,
    config: OutgoingRequestConfig,
) -> HttpResult<HostFutureIncomingResponse> {
    tracing::debug!(
        method = %request.method(),
        uri = %request.uri(),
        use_tls = config.use_tls,
        "sending gRPC request with HTTP/2 client"
    );
    let clients = match GRPC_CLIENTS.get() {
        Some(c) => c,
        None => {
            return Err(
                wasmtime_wasi_http::bindings::http::types::ErrorCode::HttpProtocolError.into(),
            );
        }
    };
    let between_bytes_timeout = config.between_bytes_timeout;

    Ok(HostFutureIncomingResponse::Pending(
        wasmtime_wasi_upstream::runtime::spawn(async move {
            match clients.request(request).await {
                Ok(resp) => Ok(Ok(IncomingResponse {
                    resp: resp.map(|body| {
                        body.map_err(|e| {
                            tracing::warn!(?e, "gRPC body error");
                            wasmtime_wasi_http::bindings::http::types::ErrorCode::HttpProtocolError
                        })
                        .boxed()
                    }),
                    worker: None,
                    between_bytes_timeout,
                })),
                Err(e) => {
                    tracing::warn!(?e, "gRPC request failed");
                    Ok(Err(
                        wasmtime_wasi_http::bindings::http::types::ErrorCode::HttpProtocolError,
                    ))
                }
            }
        }),
    ))
}
