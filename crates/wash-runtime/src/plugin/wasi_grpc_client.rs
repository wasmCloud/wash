//! gRPC client plugin for making outgoing gRPC requests.
//!
//! gRPC is built on top of HTTP/2, so this plugin leverages the same
//! connection pooling infrastructure as the HTTP client, but enforces
//! HTTP/2 and adds gRPC-specific headers.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, bail};
use hyper::body::Incoming;
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use tracing::{debug, info, warn};
use wasmtime_wasi_http::body::HyperOutgoingBody;

use crate::engine::workload::WorkloadComponent;
use crate::plugin::HostPlugin;
use crate::transport::connector::{HttpConnector, HttpsConnector};
use crate::wit::{WitInterface, WitWorld};

const GRPC_CLIENT_ID: &str = "grpc-client";

// gRPC uses HTTP/2 exclusively
const ALPN_H2: &[u8] = b"h2";

// gRPC content type
const GRPC_CONTENT_TYPE: &str = "application/grpc";

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

    /// Get a reference to the h2c client (plain TCP HTTP/2)
    pub fn h2c(&self) -> &Client<HttpConnector, HyperOutgoingBody> {
        &self.h2c
    }

    /// Get a reference to the h2 client (HTTP/2 over TLS)
    pub fn h2(&self) -> &Client<HttpsConnector, HyperOutgoingBody> {
        &self.h2
    }
}

/// gRPC client plugin for outgoing gRPC requests
///
/// This is similar to HttpClient but enforces HTTP/2 and adds gRPC-specific behavior
pub struct GrpcClient {
    /// Shared gRPC clients with automatic connection pooling
    pub clients: Arc<GrpcClients>,
    /// Configuration
    #[allow(dead_code)]
    config: HashMap<String, String>,
}

impl GrpcClient {
    /// Creates a new gRPC client instance with default configuration
    pub fn new() -> Self {
        Self::with_config(HashMap::default())
    }

    /// Creates a new gRPC client instance with custom configuration
    pub fn with_config(config: HashMap<String, String>) -> Self {
        let enable_pooling = config
            .get("enable_pooling")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);

        let tls_config = Self::configure_tls(&config).expect("failed to configure TLS for gRPC");

        let clients = Arc::new(GrpcClients::new(enable_pooling, tls_config));

        Self { clients, config }
    }

    /// Configure TLS for gRPC (requires ALPN with h2)
    fn configure_tls(
        config: &HashMap<String, String>,
    ) -> anyhow::Result<Arc<rustls::ClientConfig>> {
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
}

impl Default for GrpcClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl HostPlugin for GrpcClient {
    fn id(&self) -> &'static str {
        GRPC_CLIENT_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            // gRPC uses the same wasi:http interface but forces HTTP/2
            imports: HashSet::from([WitInterface::from("wasi:http/outgoing-handler")]),
            ..Default::default()
        }
    }

    async fn start(&self) -> anyhow::Result<()> {
        info!("starting gRPC client plugin");
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
            bail!("no wasi:http/outgoing-handler interface found for gRPC");
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
            "gRPC client plugin bound to component - using WasiHttpView::send_request override"
        );

        // NOTE: Similar to HTTP client, we rely on the engine's wasmtime_wasi_http
        // integration. We override Ctx::send_request() to use GrpcClients which
        // enforces HTTP/2 and adds gRPC headers.

        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        info!("stopping gRPC client plugin");
        Ok(())
    }
}
