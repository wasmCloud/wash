//! Tower Service connectors for HTTP and HTTPS connections

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use hyper::Uri;
use hyper_util::client::legacy::connect::{Connected, Connection};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use tower_service::Service;
use tracing::debug;

/// HTTP/2 protocol identifier for ALPN
const ALPN_H2: &[u8] = b"h2";

/// HTTP connector for plain TCP connections
#[derive(Clone)]
pub struct HttpConnector;

impl Service<Uri> for HttpConnector {
    type Response = TokioIo<TcpStream>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, uri: Uri) -> Self::Future {
        Box::pin(async move {
            let authority = uri.authority().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing authority")
            })?;

            let host_and_port = if authority.port().is_some() {
                authority.as_str().to_string()
            } else {
                format!("{}:80", authority.as_str())
            };

            debug!(host_and_port, "connecting to HTTP endpoint");
            let stream = TcpStream::connect(host_and_port).await?;
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

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
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

/// Wrapper for TLS stream that implements Connection trait
pub struct RustlsStream(TlsStream<TcpStream>);

impl Connection for RustlsStream {
    fn connected(&self) -> Connected {
        // Check ALPN negotiation result and signal to hyper
        let (_, session) = self.0.get_ref();
        if session.alpn_protocol() == Some(ALPN_H2) {
            Connected::new().negotiated_h2()
        } else {
            Connected::new()
        }
    }
}

impl tokio::io::AsyncRead for RustlsStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().0).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for RustlsStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.get_mut().0).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.get_mut().0).poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.get_mut().0).poll_shutdown(cx)
    }
}
