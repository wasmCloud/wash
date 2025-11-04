//! HTTP client plugin for making outgoing HTTP requests.
//!
//! This plugin implements the `wasi:http/outgoing-handler` interface,
//! allowing WebAssembly components to make HTTP requests to external services.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, bail};
use hyper::body::Incoming;
use hyper_util::{
    client::legacy::Client,
    rt::TokioExecutor,
};
use tracing::{debug, info, warn};
use wasmtime::component::Resource;
use wasmtime_wasi_http::body::HyperOutgoingBody;

use crate::engine::ctx::Ctx;
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

/// HTTP/2 protocol identifier for ALPN
const ALPN_H2: &[u8] = b"h2";
/// HTTP/1.1 protocol identifier for ALPN
const ALPN_HTTP11: &[u8] = b"http/1.1";

/// Shared HTTP clients with automatic connection pooling
#[derive(Clone)]
pub struct HttpClients {
    /// HTTP/1.1 client for plain TCP
    http1: Client<HttpConnector, HyperOutgoingBody>,
    /// HTTP/2 client for plain TCP (h2c - cleartext HTTP/2)
    http2: Client<HttpConnector, HyperOutgoingBody>,
    /// HTTPS client with ALPN negotiation
    https: Client<HttpsConnector, HyperOutgoingBody>,
}

impl HttpClients {
    pub fn new(enable_pooling: bool, tls_config: Arc<rustls::ClientConfig>) -> Self {
        let builder = move || {
            let mut builder = Client::builder(TokioExecutor::new());
            if !enable_pooling {
                builder.pool_max_idle_per_host(0);
            }
            builder
        };

        Self {
            http1: builder().build(HttpConnector),
            http2: builder().http2_only(true).build(HttpConnector),
            https: builder().build(HttpsConnector::new(tls_config.clone())),
        }
    }

    /// Send an HTTP request using the appropriate client
    pub async fn request(
        &self,
        request: hyper::Request<HyperOutgoingBody>,
    ) -> Result<hyper::Response<Incoming>, hyper_util::client::legacy::Error> {
        let use_tls = request.uri().scheme() == Some(&hyper::http::uri::Scheme::HTTPS);

        if use_tls {
            self.https.request(request).await
        } else {
            // Default to HTTP/1.1 for plain TCP
            self.http1.request(request).await
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

        let tls_config = Self::configure_tls(&config)
            .expect("failed to configure TLS");
        
        let clients = Arc::new(HttpClients::new(enable_pooling, tls_config));

        Self { clients, config }
    }

    /// Configure TLS based on the provided configuration
    fn configure_tls(config: &HashMap<String, String>) -> anyhow::Result<Arc<rustls::ClientConfig>> {
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
            let f = std::fs::File::open(file_path)
                .context("failed to open SSL certificate file")?;
            let mut reader = std::io::BufReader::new(f);
            let certs: Vec<rustls::pki_types::CertificateDer<'static>> = 
                rustls_pemfile::certs(&mut reader)
                    .collect::<Result<Vec<_>, _>>()
                    .context("failed to parse certificates")?;
            let (added, ignored) = ca.add_parsable_certificates(certs);
            debug!(added, ignored, file_path, "added additional root certificates");
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
            warn!(?interfaces, "ignoring non-wasi:http/outgoing-handler interfaces");
        } else if http_iface.interfaces.len() > 1 {
            warn!(?http_iface.interfaces, "ignoring non-outgoing-handler interfaces");
        }

        debug!(
            workload_id = component.id(),
            "adding HTTP outgoing-handler interface to linker"
        );
        
        let linker = component.linker();
        bindings::wasi::http::outgoing_handler::add_to_linker(linker, |ctx| ctx)?;
        
        debug!(
            workload_id = component.id(),
            "HTTP client plugin bound to component"
        );
        
        Ok(())
    }

    async fn stop(&self) -> anyhow::Result<()> {
        info!("stopping HTTP client plugin");
        Ok(())
    }
}

// Implementation of wasi:http/outgoing-handler on Ctx
// This is where the actual HTTP request handling happens
impl bindings::wasi::http::outgoing_handler::Host for Ctx {
    async fn handle(
        &mut self,
        request: Resource<bindings::wasi::http::types::OutgoingRequest>,
        options: Option<Resource<bindings::wasi::http::types::RequestOptions>>,
    ) -> anyhow::Result<Result<Resource<bindings::wasi::http::types::FutureIncomingResponse>, bindings::wasi::http::types::ErrorCode>> {
        use bindings::wasi::http::types::ErrorCode;
        
        // Get the HTTP client plugin (similar to blobstore pattern)
        let Some(plugin) = self.get_plugin::<HttpClient>(HTTP_CLIENT_ID) else {
            return Ok(Err(ErrorCode::InternalError(Some(
                "HTTP client plugin not available".to_string(),
            ))));
        };

        info!(
            workload_id = ?self.workload_id,
            component_id = ?self.component_id,
            "handling outgoing HTTP request"
        );

        // TODO: Implement WASI type conversion
        // 
        // Similar to how blobstore does it:
        // 1. Get request data from the resource table:
        //    let request_data = self.table.get(&request)?;
        //
        // 2. Convert WASI types to hyper types:
        //    - Extract method, URI, headers, body from request_data
        //    - Build hyper::Request<HyperOutgoingBody>
        //
        // 3. Send request using the plugin's clients:
        //    let response = plugin.clients.request(hyper_request).await
        //        .map_err(|e| ErrorCode::HttpProtocolError)?;
        //
        // 4. Convert hyper response back to WASI types:
        //    - Create FutureIncomingResponse from hyper::Response
        //    - Push to resource table: self.table.push(wasi_response)?
        //
        // Note: Connection pooling is automatic - no manual management needed!
        
        Ok(Err(ErrorCode::InternalError(Some(
            "WASI type conversion not yet implemented".to_string(),
        ))))
    }
}