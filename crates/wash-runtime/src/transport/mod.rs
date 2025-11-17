//! Shared transport support for HTTP/2, connection pooling, and header validation.
//!
//! This module provides utilities and abstractions for HTTP and gRPC transport plugins.
//!
//! # Features
//!
//! - HTTP/2 negotiation (ALPN)
//! - Connection pooling
//! - Header validation (TE: trailers)
//! - TLS configuration helpers
//! - Utilities for request/response handling

use anyhow::{Context, ensure};
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, ServerConfig};
use std::path::Path;
use tracing::debug;
pub mod connector;

/// HTTP/2 protocol identifier for ALPN
pub const ALPN_H2: &[u8] = b"h2";
/// HTTP/1.1 protocol identifier for ALPN
pub const ALPN_HTTP11: &[u8] = b"http/1.1";

/// Negotiated HTTP protocol version from ALPN
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpVersion {
    /// HTTP/1.1
    Http11,
    /// HTTP/2
    Http2,
}

/// Determines the HTTP version from ALPN protocol negotiation result.
///
/// # Arguments
/// * `alpn_protocol` - The ALPN protocol bytes negotiated during TLS handshake
///
/// # Returns
/// The negotiated HTTP version, defaulting to HTTP/1.1 if not recognized
pub fn negotiate_http_version(alpn_protocol: Option<&[u8]>) -> HttpVersion {
    match alpn_protocol {
        Some(ALPN_H2) => {
            debug!("negotiated HTTP/2 via ALPN");
            HttpVersion::Http2
        }
        Some(ALPN_HTTP11) => {
            debug!("negotiated HTTP/1.1 via ALPN");
            HttpVersion::Http11
        }
        Some(other) => {
            debug!(
                protocol = ?String::from_utf8_lossy(other),
                "unknown ALPN protocol, defaulting to HTTP/1.1"
            );
            HttpVersion::Http11
        }
        None => {
            debug!("no ALPN protocol negotiated, defaulting to HTTP/1.1");
            HttpVersion::Http11
        }
    }
}

/// Validates that the TE header is only 'trailers'.
///
/// According to HTTP specifications, the TE header can only have the value 'trailers'
/// when used with HTTP/2 or in certain HTTP/1.1 contexts. This function ensures compliance
/// with the specification.
///
/// # Arguments
/// * `headers` - The HTTP header map to validate
///
/// # Returns
/// `Ok(())` if the TE header is valid or not present, `Err` if it contains invalid values
///
/// # Errors
/// Returns an error if the TE header is present and not equal to 'trailers'
pub fn validate_te_header(headers: &hyper::header::HeaderMap) -> anyhow::Result<()> {
    if let Some(te) = headers.get("te") {
        let te_str = te
            .to_str()
            .context("TE header contains invalid characters")?;

        // The TE header must only be "trailers"
        ensure!(
            te_str.trim().eq_ignore_ascii_case("trailers"),
            "TE header must be 'trailers', got: {te_str}"
        );

        debug!("validated TE header: trailers");
    }
    Ok(())
}

/// Configures a TLS client with root certificates and ALPN support.
///
/// # Arguments
/// * `load_native_certs` - Whether to load native system root certificates
/// * `load_webpki_certs` - Whether to load Mozilla WebPKI root certificates
/// * `cert_file` - Optional path to additional certificate file
///
/// # Returns
/// A configured `ClientConfig` ready for use with TLS connections
///
/// # Errors
/// Returns an error if certificate loading fails
pub fn configure_tls_client(
    load_native_certs: bool,
    load_webpki_certs: bool,
    cert_file: Option<&Path>,
) -> anyhow::Result<ClientConfig> {
    // Install default crypto provider if not already installed
    // This is required for rustls 0.23+
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let mut ca = rustls::RootCertStore::empty();

    // Load native certificates
    if load_native_certs {
        // Note: Native certificate loading would require platform-specific code
        // For now, we'll skip this and rely on webpki
        debug!("native certificate loading not yet fully implemented");
    }

    // Load Mozilla trusted root certificates
    if load_webpki_certs {
        ca.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        debug!("loaded webpki root certificate store");
    }

    // Load root certificates from a file
    if let Some(cert_path) = cert_file {
        let f = std::fs::File::open(cert_path)
            .with_context(|| format!("failed to open certificate file: {}", cert_path.display()))?;
        let mut reader = std::io::BufReader::new(f);
        let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
            .collect::<Result<Vec<_>, _>>()
            .with_context(|| {
                format!("failed to parse certificate file: {}", cert_path.display())
            })?;
        let (added, ignored) = ca.add_parsable_certificates(certs);
        debug!(
            added,
            ignored,
            file = %cert_path.display(),
            "added additional root certificates from file"
        );
    }

    // Configure ALPN protocols
    let mut tls_config = ClientConfig::builder()
        .with_root_certificates(ca)
        .with_no_client_auth();

    // Set ALPN protocols (HTTP/2 first, then HTTP/1.1)
    tls_config.alpn_protocols = vec![ALPN_H2.to_vec(), ALPN_HTTP11.to_vec()];

    debug!("TLS client configured with ALPN support");
    Ok(tls_config)
}

/// Configures a TLS server with certificates and ALPN support.
///
/// # Arguments
/// * `cert_path` - Path to the server certificate file
/// * `key_path` - Path to the private key file
/// * `enable_http2` - Whether to enable HTTP/2 via ALPN
///
/// # Returns
/// A configured `ServerConfig` ready for use with TLS connections
///
/// # Errors
/// Returns an error if certificate or key loading fails
pub async fn configure_tls_server(
    cert_path: &Path,
    key_path: &Path,
    enable_http2: bool,
) -> anyhow::Result<ServerConfig> {
    // Install default crypto provider if not already installed
    // This is required for rustls 0.23+
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Load certificate chain
    let cert_data = tokio::fs::read(cert_path)
        .await
        .with_context(|| format!("failed to read certificate file: {}", cert_path.display()))?;
    let mut cert_reader = std::io::Cursor::new(cert_data);
    let cert_chain: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("failed to parse certificate file: {}", cert_path.display()))?;

    ensure!(
        !cert_chain.is_empty(),
        "no certificates found in file: {}",
        cert_path.display()
    );

    // Load private key
    let key_data = tokio::fs::read(key_path)
        .await
        .with_context(|| format!("failed to read private key file: {}", key_path.display()))?;
    let mut key_reader = std::io::Cursor::new(key_data);
    let key = rustls_pemfile::private_key(&mut key_reader)
        .with_context(|| format!("failed to parse private key file: {}", key_path.display()))?
        .ok_or_else(|| anyhow::anyhow!("no private key found in file: {}", key_path.display()))?;

    // Create rustls server config
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)
        .context("failed to create TLS configuration")?;

    // Configure ALPN protocols if HTTP/2 is enabled
    if enable_http2 {
        config.alpn_protocols = vec![ALPN_H2.to_vec(), ALPN_HTTP11.to_vec()];
        debug!("TLS server configured with HTTP/2 ALPN support");
    } else {
        config.alpn_protocols = vec![ALPN_HTTP11.to_vec()];
        debug!("TLS server configured with HTTP/1.1 only");
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_negotiate_http_version() {
        assert_eq!(negotiate_http_version(Some(ALPN_H2)), HttpVersion::Http2);
        assert_eq!(
            negotiate_http_version(Some(ALPN_HTTP11)),
            HttpVersion::Http11
        );
        assert_eq!(
            negotiate_http_version(Some(b"unknown")),
            HttpVersion::Http11
        );
        assert_eq!(negotiate_http_version(None), HttpVersion::Http11);
    }

    #[test]
    fn test_validate_te_header() {
        let mut headers = hyper::header::HeaderMap::new();

        // No TE header should be valid
        assert!(validate_te_header(&headers).is_ok());

        // TE: trailers should be valid
        headers.insert("te", "trailers".parse().unwrap());
        assert!(validate_te_header(&headers).is_ok());

        // TE: trailers (with whitespace) should be valid
        headers.insert("te", "  trailers  ".parse().unwrap());
        assert!(validate_te_header(&headers).is_ok());

        // TE: chunked should be invalid
        headers.insert("te", "chunked".parse().unwrap());
        assert!(validate_te_header(&headers).is_err());

        // TE: gzip should be invalid
        headers.insert("te", "gzip".parse().unwrap());
        assert!(validate_te_header(&headers).is_err());
    }

    #[test]
    fn test_configure_tls_client() {
        // Should succeed with webpki certs
        let config = configure_tls_client(false, true, None);
        assert!(config.is_ok());

        let config = config.unwrap();
        // Should have ALPN protocols configured
        assert_eq!(config.alpn_protocols.len(), 2);
        assert_eq!(config.alpn_protocols[0], ALPN_H2);
        assert_eq!(config.alpn_protocols[1], ALPN_HTTP11);
    }
}
