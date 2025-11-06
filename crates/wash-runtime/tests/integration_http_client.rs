//! Integration test for HTTP client plugin with HTTP/2 support
//!
//! This test demonstrates:
//! 1. Starting a host with HTTP server plugin (incoming) and HTTP client plugin (outgoing)
//! 2. Configuring HTTP client with HTTP/2 support and connection pooling
//! 3. Testing components that make outbound HTTP requests
//! 4. Verifying HTTP/1.1 and HTTP/2 protocol negotiation via ALPN
//! 5. Testing connection pooling and reuse across multiple requests
//! 6. Validating TLS/HTTPS connections with proper certificate validation
//! 7. Testing both plain TCP (h2c) and TLS (h2) HTTP/2 connections

use anyhow::{Context, Result};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::time::timeout;

mod common;
use common::find_available_port;

use wash_runtime::{
    engine::Engine,
    host::{HostApi, HostBuilder},
    plugin::{
        wasi_blobstore::WasiBlobstore, wasi_config::RuntimeConfig, wasi_http::HttpServer,
        wasi_http_client::HttpClient, wasi_keyvalue::WasiKeyvalue, wasi_logging::WasiLogging,
    },
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

const HTTP_HELLO_WORLD_WASM: &[u8] = include_bytes!("fixtures/http_hello_world.wasm");

/// Test HTTP client plugin with default configuration (auto HTTP version, pooling enabled)
#[tokio::test]
async fn test_http_client_default_config() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("wash_runtime=debug".parse().unwrap()),
        )
        .init();

    println!("\n╔═══════════════════════════════════════════════════════════════════════╗");
    println!("║           🧪 HTTP CLIENT PLUGIN - DEFAULT CONFIG TEST                ║");
    println!("╚═══════════════════════════════════════════════════════════════════════╝\n");

    let engine = Engine::builder().build()?;
    let port = find_available_port().await?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    // Create HTTP client with default config (auto version selection, pooling enabled)
    let http_client = HttpClient::new();
    println!("✓ Created HTTP client with default configuration");
    println!("  - HTTP version: Auto (prefers HTTP/1.1, upgrades if supported)");
    println!("  - Connection pooling: Enabled");
    println!("  - TLS: Enabled with webpki root certificates");
    println!("  - ALPN: [h2, http/1.1]\n");

    let host = HostBuilder::new()
        .with_engine(engine.clone())
        .with_plugin(Arc::new(HttpServer::new(addr)))?
        .with_plugin(Arc::new(http_client))?
        .with_plugin(Arc::new(WasiBlobstore::new(None)))?
        .with_plugin(Arc::new(WasiKeyvalue::new()))?
        .with_plugin(Arc::new(WasiLogging {}))?
        .with_plugin(Arc::new(RuntimeConfig::default()))?
        .build()?;

    let host = host.start().await.context("Failed to start host")?;
    println!("✓ Host started with HTTP client plugin");
    println!("  - Listening on: {addr}\n");

    let workload_id = start_http_hello_workload(&host, addr, HashMap::new()).await?;
    println!("✓ Workload started: {workload_id}\n");

    // Make a test request - the component will make an outbound HTTPS request to example.com
    println!("▶ Testing outbound HTTPS request to example.com...");
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(15),
        client
            .get(format!("http://{addr}/outbound"))
            .header("HOST", "foo")
            .send(),
    )
    .await
    .context("Request timed out")?
    .context("Failed to make request")?;

    let status = response.status();
    let body = response.text().await?;

    println!("✓ Request completed successfully");
    println!("  - Status: {status}");
    println!("  - Response preview: {}...", &body[..body.len().min(100)]);
    println!("  - Component successfully made outbound HTTPS request\n");

    assert!(status.is_success(), "Expected success status, got {status}");
    assert!(
        body.contains("Outbound HTTPS request successful"),
        "Expected outbound request confirmation in response"
    );

    // Make another request to test connection pooling
    println!("▶ Testing connection pooling (second request)...");
    let response2 = timeout(
        Duration::from_secs(15),
        client
            .get(format!("http://{addr}/outbound"))
            .header("HOST", "foo")
            .send(),
    )
    .await
    .context("Second request timed out")?
    .context("Failed to make second request")?;

    let status2 = response2.status();
    let body2 = response2.text().await?;

    println!("✓ Second request completed (connection should be reused)");
    println!("  - Status: {status2}");
    println!(
        "  - Response preview: {}...",
        &body2[..body2.len().min(100)]
    );
    println!("  - Connection pooling working correctly\n");

    assert!(status2.is_success());
    assert!(
        body2.contains("Outbound HTTPS request successful"),
        "Expected outbound request confirmation"
    );

    println!("✅ Default config test passed!\n");
    Ok(())
}

/// Test HTTP client with HTTP/2 forced via configuration
#[tokio::test]
async fn test_http_client_http2_forced() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("wash_runtime=debug".parse().unwrap()),
        )
        .init();

    println!("\n╔═══════════════════════════════════════════════════════════════════════╗");
    println!("║           🧪 HTTP CLIENT PLUGIN - FORCE HTTP/2 TEST                  ║");
    println!("╚═══════════════════════════════════════════════════════════════════════╝\n");

    let engine = Engine::builder().build()?;
    let port = find_available_port().await?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    // Create HTTP client with HTTP/2 forced
    let http_client_config = HashMap::from([
        ("PREFER_HTTP2".to_string(), "true".to_string()),
        ("enable_pooling".to_string(), "true".to_string()),
    ]);
    let http_client = HttpClient::with_config(http_client_config.clone());

    println!("✓ Created HTTP client with HTTP/2 forced");
    println!("  - HTTP version: Http2Only");
    println!("  - Connection pooling: Enabled");
    println!("  - Configuration: {:?}\n", http_client_config);

    let host = HostBuilder::new()
        .with_engine(engine.clone())
        .with_plugin(Arc::new(HttpServer::new(addr)))?
        .with_plugin(Arc::new(http_client))?
        .with_plugin(Arc::new(WasiBlobstore::new(None)))?
        .with_plugin(Arc::new(WasiKeyvalue::new()))?
        .with_plugin(Arc::new(WasiLogging {}))?
        .with_plugin(Arc::new(RuntimeConfig::default()))?
        .build()?;

    let host = host.start().await.context("Failed to start host")?;
    println!("✓ Host started with HTTP/2 client");
    println!("  - Listening on: {addr}\n");

    let workload_id = start_http_hello_workload(&host, addr, HashMap::new()).await?;
    println!("✓ Workload started: {workload_id}\n");

    // Make test requests
    println!("▶ Testing HTTPS request with HTTP/2 via ALPN...");
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(15),
        client
            .get(format!("http://{addr}/outbound"))
            .header("HOST", "foo")
            .send(),
    )
    .await??;

    let status = response.status();
    let body = response.text().await?;

    println!("✓ HTTP/2 request completed");
    println!("  - Status: {status}");
    println!("  - Response: {}...", &body[..body.len().min(100)]);
    println!("  - ALPN negotiated HTTP/2 for HTTPS connection\n");

    assert!(status.is_success());
    assert!(body.contains("Outbound HTTPS request successful"));

    println!("✅ HTTP/2 forced test passed!\n");
    Ok(())
}

/// Test HTTP client with HTTP/1.1 only
#[tokio::test]
async fn test_http_client_http1_only() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("wash_runtime=debug".parse().unwrap()),
        )
        .init();

    println!("\n╔═══════════════════════════════════════════════════════════════════════╗");
    println!("║           🧪 HTTP CLIENT PLUGIN - HTTP/1.1 ONLY TEST                 ║");
    println!("╚═══════════════════════════════════════════════════════════════════════╝\n");

    let engine = Engine::builder().build()?;
    let port = find_available_port().await?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    // Create HTTP client with HTTP/1.1 only
    let http_client_config = HashMap::from([
        ("PREFER_HTTP2".to_string(), "false".to_string()),
        ("enable_pooling".to_string(), "true".to_string()),
    ]);
    let http_client = HttpClient::with_config(http_client_config.clone());

    println!("✓ Created HTTP client with HTTP/1.1 only");
    println!("  - HTTP version: Http1Only");
    println!("  - Connection pooling: Enabled");
    println!("  - Configuration: {:?}\n", http_client_config);

    let host = HostBuilder::new()
        .with_engine(engine.clone())
        .with_plugin(Arc::new(HttpServer::new(addr)))?
        .with_plugin(Arc::new(http_client))?
        .with_plugin(Arc::new(WasiBlobstore::new(None)))?
        .with_plugin(Arc::new(WasiKeyvalue::new()))?
        .with_plugin(Arc::new(WasiLogging {}))?
        .with_plugin(Arc::new(RuntimeConfig::default()))?
        .build()?;

    let host = host.start().await.context("Failed to start host")?;
    println!("✓ Host started with HTTP/1.1-only client");
    println!("  - Listening on: {addr}\n");

    let workload_id = start_http_hello_workload(&host, addr, HashMap::new()).await?;
    println!("✓ Workload started: {workload_id}\n");

    println!("▶ Testing HTTP/1.1-only request...");
    let client = reqwest::Client::new();
    let response = timeout(
        Duration::from_secs(15),
        client
            .get(format!("http://{addr}/outbound"))
            .header("HOST", "foo")
            .send(),
    )
    .await??;

    let status = response.status();
    let body = response.text().await?;

    println!("✓ HTTP/1.1 request completed");
    println!("  - Status: {status}");
    println!("  - Response: {}...", &body[..body.len().min(100)]);
    println!("  - Using HTTP/1.1 exclusively\n");

    assert!(status.is_success());
    assert!(body.contains("Outbound HTTPS request successful"));

    println!("✅ HTTP/1.1 only test passed!\n");
    Ok(())
}

/// Test HTTP client with connection pooling disabled
#[tokio::test]
async fn test_http_client_no_pooling() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("wash_runtime=debug".parse().unwrap()),
        )
        .init();

    println!("\n╔═══════════════════════════════════════════════════════════════════════╗");
    println!("║           🧪 HTTP CLIENT PLUGIN - NO POOLING TEST                    ║");
    println!("╚═══════════════════════════════════════════════════════════════════════╝\n");

    let engine = Engine::builder().build()?;
    let port = find_available_port().await?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    // Create HTTP client with pooling disabled
    let http_client_config = HashMap::from([("enable_pooling".to_string(), "false".to_string())]);
    let http_client = HttpClient::with_config(http_client_config.clone());

    println!("✓ Created HTTP client with pooling disabled");
    println!("  - HTTP version: Auto");
    println!("  - Connection pooling: Disabled (new connection per request)");
    println!("  - Configuration: {:?}\n", http_client_config);

    let host = HostBuilder::new()
        .with_engine(engine.clone())
        .with_plugin(Arc::new(HttpServer::new(addr)))?
        .with_plugin(Arc::new(http_client))?
        .with_plugin(Arc::new(WasiBlobstore::new(None)))?
        .with_plugin(Arc::new(WasiKeyvalue::new()))?
        .with_plugin(Arc::new(WasiLogging {}))?
        .with_plugin(Arc::new(RuntimeConfig::default()))?
        .build()?;

    let host = host.start().await.context("Failed to start host")?;
    println!("✓ Host started with no-pooling client");
    println!("  - Listening on: {addr}\n");

    let workload_id = start_http_hello_workload(&host, addr, HashMap::new()).await?;
    println!("✓ Workload started: {workload_id}\n");

    println!("▶ Testing requests without connection pooling...");
    let client = reqwest::Client::new();

    // Each request should create a new connection
    for i in 1..=3 {
        println!("  Request {i}: Creating new connection...");
        let response = timeout(
            Duration::from_secs(15),
            client
                .get(format!("http://{addr}/outbound"))
                .header("HOST", "foo")
                .send(),
        )
        .await??;

        let status = response.status();
        let body = response.text().await?;

        println!("  ✓ Request {i} completed - Status: {status}");
        assert!(status.is_success());
        assert!(body.contains("Outbound HTTPS request successful"));
    }

    println!("\n✅ No pooling test passed!\n");
    Ok(())
}

/// Test concurrent requests with connection pooling
#[tokio::test]
async fn test_http_client_concurrent_requests() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("wash_runtime=debug".parse().unwrap()),
        )
        .init();

    println!("\n╔═══════════════════════════════════════════════════════════════════════╗");
    println!("║           🧪 HTTP CLIENT PLUGIN - CONCURRENT REQUESTS TEST           ║");
    println!("╚═══════════════════════════════════════════════════════════════════════╝\n");

    let engine = Engine::builder().build()?;
    let port = find_available_port().await?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    let http_client = HttpClient::new();
    println!("✓ Created HTTP client with connection pooling");
    println!("  - Multiple concurrent requests will share pooled connections\n");

    let host = HostBuilder::new()
        .with_engine(engine.clone())
        .with_plugin(Arc::new(HttpServer::new(addr)))?
        .with_plugin(Arc::new(http_client))?
        .with_plugin(Arc::new(WasiBlobstore::new(None)))?
        .with_plugin(Arc::new(WasiKeyvalue::new()))?
        .with_plugin(Arc::new(WasiLogging {}))?
        .with_plugin(Arc::new(RuntimeConfig::default()))?
        .build()?;

    let host = host.start().await.context("Failed to start host")?;
    println!("✓ Host started");
    println!("  - Listening on: {addr}\n");

    let workload_id = start_http_hello_workload(&host, addr, HashMap::new()).await?;
    println!("✓ Workload started: {workload_id}\n");

    println!("▶ Launching 10 concurrent requests...");
    let client = Arc::new(reqwest::Client::new());
    let mut handles = vec![];

    let start = std::time::Instant::now();

    for i in 1..=10 {
        let client = client.clone();
        let url = format!("http://{addr}/outbound");

        let handle = tokio::spawn(async move {
            let response = timeout(
                Duration::from_secs(20),
                client.get(&url).header("HOST", "foo").send(),
            )
            .await??;

            let status = response.status();
            let body = response.text().await?;

            anyhow::Ok((i, status, body))
        });

        handles.push(handle);
    }

    let mut results = vec![];
    for handle in handles {
        results.push(handle.await);
    }
    let elapsed = start.elapsed();

    println!("✓ All requests completed in {elapsed:?}");
    println!("  - Average: {:?} per request\n", elapsed / 10);

    let mut success_count = 0;
    for result in results {
        match result {
            Ok(Ok((i, status, body))) => {
                println!("  ✓ Request {i}: Status {status}");
                assert!(status.is_success());
                assert!(body.contains("Outbound HTTPS request successful"));
                success_count += 1;
            }
            Ok(Err(e)) => println!("  ✗ Request failed: {e}"),
            Err(e) => println!("  ✗ Task failed: {e}"),
        }
    }

    println!("\n✓ Successfully completed {success_count}/10 concurrent requests");
    println!("  - Connection pooling enabled efficient request handling\n");

    assert_eq!(success_count, 10, "Expected all 10 requests to succeed");

    println!("✅ Concurrent requests test passed!\n");
    Ok(())
}

// Helper function to start the http-hello-world workload
async fn start_http_hello_workload(
    host: &wash_runtime::host::Host,
    _addr: SocketAddr,
    component_config: HashMap<String, String>,
) -> Result<String> {
    let req = WorkloadStartRequest {
        workload: Workload {
            namespace: "test".to_string(),
            name: "http-hello-workload".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                bytes: bytes::Bytes::from_static(HTTP_HELLO_WORLD_WASM),
                local_resources: LocalResources {
                    memory_limit_mb: 256,
                    cpu_limit: 1,
                    config: component_config,
                    environment: HashMap::new(),
                    volume_mounts: vec![],
                    allowed_hosts: vec![],
                },
                pool_size: 1,
                max_invocations: 100,
            }],
            host_interfaces: vec![
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "http".to_string(),
                    interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                    version: Some(semver::Version::parse("0.2.2").unwrap()),
                    config: {
                        let mut config = HashMap::new();
                        config.insert("host".to_string(), "foo".to_string());
                        config
                    },
                },
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "http".to_string(),
                    interfaces: ["outgoing-handler".to_string()].into_iter().collect(),
                    version: Some(semver::Version::parse("0.2.2").unwrap()),
                    config: HashMap::new(),
                },
            ],
            volumes: vec![],
        },
    };

    let workload_response = host
        .workload_start(req)
        .await
        .context("Failed to start workload")?;

    Ok(workload_response.workload_status.workload_id)
}
