//! HTTP client plugin for making outgoing HTTP requests.
//!
//! This plugin implements the `wasi:http/outgoing-handler` interface,
//! allowing WebAssembly components to make HTTP requests to external services.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, bail};
use hyper::body::Incoming;
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use tracing::{debug, info, warn};
use wasmtime_wasi_http::body::HyperOutgoingBody;

use crate::engine::workload::WorkloadComponent;
use crate::plugin::HostPlugin;
use crate::wit::{WitInterface, WitWorld};

use crate::transport::connector::{HttpConnector, HttpsConnector};

const HTTP_CLIENT_ID: &str = "http-client";

mod bindings {
    wasmtime::component::bindgen!({
        world: "http-client",
        trappable_imports: true,
        async: true,
        with: {
            "wasi:io/streams": wasmtime_wasi::bindings::io::streams,
            "wasi:io/poll": wasmtime_wasi::bindings::io::poll,
        },
    });
}

// Configuration constants
const LOAD_WEBPKI_CERTS: &str = "load_webpki_certs";
const SSL_CERTS_FILE: &str = "ssl_certs_file";
const ENABLE_POOLING: &str = "enable_pooling";
const PREFER_HTTP2: &str = "prefer_http2"; // New: prefer HTTP/2 for plain TCP

/// HTTP/2 protocol identifier for ALPN
const ALPN_H2: &[u8] = b"h2";
/// HTTP/1.1 protocol identifier for ALPN
const ALPN_HTTP11: &[u8] = b"http/1.1";

/// HTTP version preference
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HttpVersion {
    /// Always use HTTP/1.1
    Http1Only,
    /// Always use HTTP/2 (h2c for plain TCP, h2 for TLS)
    Http2Only,
    /// Use HTTP/1.1 for plain TCP, ALPN negotiation for TLS (default)
    #[default]
    Auto,
}

/// Shared HTTP clients with automatic connection pooling
#[derive(Clone)]
pub struct HttpClients {
    /// HTTP/1.1 client for plain TCP
    http1: Client<HttpConnector, HyperOutgoingBody>,
    /// HTTP/2 client for plain TCP (h2c - cleartext HTTP/2)
    http2: Client<HttpConnector, HyperOutgoingBody>,
    /// HTTPS client with ALPN negotiation
    https: Client<HttpsConnector, HyperOutgoingBody>,
    /// HTTP version preference
    version: HttpVersion,
}

impl HttpClients {
    pub fn new(
        enable_pooling: bool,
        tls_config: Arc<rustls::ClientConfig>,
        version: HttpVersion,
    ) -> Self {
        let builder = move || {
            let mut builder = Client::builder(TokioExecutor::new());
            if !enable_pooling {
                builder.pool_max_idle_per_host(0);
            }
            builder
        };

        Self {
            // HTTP/1.1 only client
            http1: builder().build(HttpConnector),
            // HTTP/2 client - hyper will use HTTP/2 when the connection is established
            http2: builder().build(HttpConnector),
            // HTTPS with ALPN - negotiates HTTP/1.1 or HTTP/2 based on server support
            https: builder().build(HttpsConnector::new(tls_config.clone())),
            version,
        }
    }

    /// Send an HTTP request using the appropriate client
    pub async fn request(
        &self,
        request: hyper::Request<HyperOutgoingBody>,
    ) -> Result<hyper::Response<Incoming>, hyper_util::client::legacy::Error> {
        let use_tls = request.uri().scheme() == Some(&hyper::http::uri::Scheme::HTTPS);

        if use_tls {
            // TLS: Always use HTTPS client with ALPN negotiation
            self.https.request(request).await
        } else {
            // Plain TCP: Use version preference
            match self.version {
                HttpVersion::Http1Only => {
                    tracing::debug!("using HTTP/1.1 for plain TCP request");
                    self.http1.request(request).await
                }
                HttpVersion::Http2Only => {
                    tracing::debug!("using HTTP/2 (h2c) for plain TCP request");
                    self.http2.request(request).await
                }
                HttpVersion::Auto => {
                    // Default: HTTP/1.1 for plain TCP
                    tracing::debug!("using HTTP/1.1 (auto) for plain TCP request");
                    self.http1.request(request).await
                }
            }
        }
    }

    /// Get a reference to the HTTP/1.1 client
    pub fn http1(&self) -> &Client<HttpConnector, HyperOutgoingBody> {
        &self.http1
    }

    /// Get a reference to the HTTP/2 client
    pub fn http2(&self) -> &Client<HttpConnector, HyperOutgoingBody> {
        &self.http2
    }

    /// Get a reference to the HTTPS client
    pub fn https(&self) -> &Client<HttpsConnector, HyperOutgoingBody> {
        &self.https
    }
}

/// HTTP client plugin for outgoing HTTP requests
///
/// This follows the same pattern as WasiBlobstore:
/// - Stores shared state (HTTP clients with connection pooling)
/// - Ctx implements the Host traits and accesses this via get_plugin()
/// - No Tower Service implementation needed - that's for component-side code
pub struct HttpClient {
    /// Shared HTTP clients with automatic connection pooling
    /// Similar to WasiBlobstore's storage: Arc<RwLock<...>>
    pub clients: Arc<HttpClients>,
    /// Configuration
    #[allow(dead_code)]
    config: HashMap<String, String>,
}

impl HttpClient {
    /// Creates a new HTTP client instance with default configuration
    pub fn new() -> Self {
        Self::with_config(HashMap::default())
    }

    /// Creates a new HTTP client instance with custom configuration
    pub fn with_config(config: HashMap<String, String>) -> Self {
        let enable_pooling = config
            .get(ENABLE_POOLING)
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);

        let version = match config.get(PREFER_HTTP2).map(|s| s.as_str()) {
            Some("true") | Some("h2") | Some("http2") => HttpVersion::Http2Only,
            Some("false") | Some("h1") | Some("http1") => HttpVersion::Http1Only,
            _ => HttpVersion::Auto,
        };

        let tls_config = Self::configure_tls(&config).expect("failed to configure TLS");

        let clients = Arc::new(HttpClients::new(enable_pooling, tls_config, version));

        Self { clients, config }
    }

    /// Configure TLS based on the provided configuration
    fn configure_tls(
        config: &HashMap<String, String>,
    ) -> anyhow::Result<Arc<rustls::ClientConfig>> {
        // This is required for rustls 0.23+
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        let mut ca = rustls::RootCertStore::empty();

        // Load Mozilla trusted root certificates
        if config
            .get(LOAD_WEBPKI_CERTS)
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(true)
        {
            ca.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            debug!("loaded webpki root certificate store");
        }

        // Load root certificates from a file
        if let Some(file_path) = config.get(SSL_CERTS_FILE) {
            let f =
                std::fs::File::open(file_path).context("failed to open SSL certificate file")?;
            let mut reader = std::io::BufReader::new(f);
            let certs: Vec<rustls::pki_types::CertificateDer<'static>> =
                rustls_pemfile::certs(&mut reader)
                    .collect::<Result<Vec<_>, _>>()
                    .context("failed to parse certificates")?;
            let (added, ignored) = ca.add_parsable_certificates(certs);
            debug!(
                added,
                ignored, file_path, "added additional root certificates"
            );
        }

        // Configure ALPN protocols (HTTP/2 first, then HTTP/1.1)
        let mut tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(ca)
            .with_no_client_auth();

        tls_config.alpn_protocols = vec![ALPN_H2.to_vec(), ALPN_HTTP11.to_vec()];

        Ok(Arc::new(tls_config))
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl HostPlugin for HttpClient {
    fn id(&self) -> &'static str {
        HTTP_CLIENT_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from("wasi:http/outgoing-handler")]),
            ..Default::default()
        }
    }

    async fn start(&self) -> anyhow::Result<()> {
        info!("starting HTTP client plugin");
        Ok(())
    }

    async fn on_component_bind(
        &self,
        component: &mut WorkloadComponent,
        interfaces: HashSet<WitInterface>,
    ) -> anyhow::Result<()> {
        let Some(http_iface) = interfaces.iter().find(|iface| {
            iface.namespace == "wasi"
                && iface.package == "http"
                && iface.interfaces.contains("outgoing-handler")
        }) else {
            bail!("no wasi:http/outgoing-handler interface found");
        };

        if interfaces.len() > 1 {
            warn!(
                ?interfaces,
                "ignoring non-wasi:http/outgoing-handler interfaces"
            );
        } else if http_iface.interfaces.len() > 1 {
            warn!(?http_iface.interfaces, "ignoring non-outgoing-handler interfaces");
        }

        debug!(
            workload_id = component.id(),
            "HTTP client plugin bound to component - using WasiHttpView::send_request override"
        );

        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        info!("stopping HTTP client plugin");
        Ok(())
    }
}
