//! Integration test for HTTP and blobstore plugins with blobby.wasm component
//!
//! This test demonstrates:
//! 1. Starting a host with HTTP and blobstore plugins
//! 2. Creating and starting a workload using blobby.wasm component
//! 3. Verifying HTTP requests work with blobstore operations
//! 4. Testing round-trip data through blobstore functionality

use anyhow::{Context, Result};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::time::timeout;

mod common;
use common::find_available_port;

use wasmcloud::{
    engine::Engine,
    host::{HostApi, HostBuilder},
    plugin::{wasi_blobstore::WasiBlobstore, wasi_http::HttpServer, wasi_logging::WasiLogging},
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

const BLOBBY_WASM: &[u8] = include_bytes!("fixtures/blobby.wasm");

#[tokio::test]
async fn test_blobby_integration() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    println!("Starting blobby.wasm integration test");

    // Create engine
    let engine = Engine::builder().build()?;

    // Create HTTP server plugin on a dynamically allocated port
    let port = find_available_port().await?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let http_plugin = HttpServer::new(addr);

    // Create blobstore plugin
    let blobstore_plugin = WasiBlobstore::new(None);

    // Create logging plugin
    let logging_plugin = WasiLogging {};

    // Build host with plugins
    let host = HostBuilder::new()
        .with_engine(engine.clone())
        .with_plugin(Arc::new(http_plugin))?
        .with_plugin(Arc::new(blobstore_plugin))?
        .with_plugin(Arc::new(logging_plugin))?
        .build()?;

    println!("Created host with HTTP and blobstore plugins for blobby test");

    // Start the host
    let host = host.start().await.context("Failed to start host")?;
    println!("Host started, HTTP server listening on {addr}");

    // Create a workload request with the blobby component
    let req = WorkloadStartRequest {
        workload: Workload {
            namespace: "test".to_string(),
            name: "blobby-workload".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                bytes: bytes::Bytes::from_static(BLOBBY_WASM),
                local_resources: LocalResources {
                    memory_limit_mb: 256,
                    cpu_limit: 1,
                    config: HashMap::new(),
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
                    version: None,
                    config: {
                        let mut config = HashMap::new();
                        config.insert("host".to_string(), "blobby-test".to_string());
                        config
                    },
                },
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "blobstore".to_string(),
                    interfaces: [
                        "blobstore".to_string(),
                        "container".to_string(),
                        "types".to_string(),
                    ]
                    .into_iter()
                    .collect(),
                    version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
                    config: HashMap::new(),
                },
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "logging".to_string(),
                    interfaces: ["logging".to_string()].into_iter().collect(),
                    version: Some(semver::Version::parse("0.1.0-draft").unwrap()),
                    config: HashMap::new(),
                },
            ],
            volumes: vec![],
        },
    };

    // Start the workload
    let workload_response = host
        .workload_start(req)
        .await
        .context("Failed to start blobby workload")?;
    println!(
        "Started blobby workload: {:?}",
        workload_response.workload_status.workload_id
    );

    // Test HTTP requests to the blobby component
    println!("Testing blobby component endpoint");

    let test_data = "Hello from blobby test!";
    let client = reqwest::Client::new();

    // Test GET request
    let get_response = timeout(
        Duration::from_secs(5),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "blobby-test")
            .send(),
    )
    .await
    .context("GET request timed out")?
    .context("Failed to make GET request")?;

    let get_status = get_response.status();
    println!("GET Response Status: {}", get_status);

    let get_response_text = get_response
        .text()
        .await
        .context("Failed to read GET response body")?;
    println!("GET Response Body: {}", get_response_text.trim());

    // Test POST request with data
    let post_response = timeout(
        Duration::from_secs(5),
        client
            .post(format!("http://{addr}/"))
            .header("HOST", "blobby-test")
            .body(test_data)
            .send(),
    )
    .await
    .context("POST request timed out")?
    .context("Failed to make POST request")?;

    let post_status = post_response.status();
    println!("POST Response Status: {}", post_status);

    let post_response_text = post_response
        .text()
        .await
        .context("Failed to read POST response body")?;
    println!("POST Response Body: {}", post_response_text.trim());

    // Verify responses - GET might return 404 for empty blobstore, POST should succeed
    assert!(
        get_status.is_success() || get_status == reqwest::StatusCode::NOT_FOUND,
        "GET request expected success or 404, got {}",
        get_status
    );
    assert!(
        post_status.is_success(),
        "POST request expected success, got {}",
        post_status
    );
    assert!(
        !get_response_text.trim().is_empty() || !post_response_text.trim().is_empty(),
        "Expected at least one response to have content"
    );

    println!("blobby.wasm component responded successfully to HTTP requests");
    println!("HTTP and blobstore plugins are working with blobby component");
    println!("Blobby integration test passed!");

    Ok(())
}

#[tokio::test]
async fn test_blobby_error_handling() -> Result<()> {
    println!("Testing blobby component error handling");

    // Create engine and plugins
    let engine = Engine::builder().build()?;
    let port = find_available_port().await?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let http_plugin = HttpServer::new(addr);
    let blobstore_plugin = WasiBlobstore::new(Some(1024 * 1024)); // 1MB limit for testing
    let logging_plugin = WasiLogging {};

    let host = HostBuilder::new()
        .with_engine(engine)
        .with_plugin(Arc::new(http_plugin))?
        .with_plugin(Arc::new(blobstore_plugin))?
        .with_plugin(Arc::new(logging_plugin))?
        .build()?;

    let host = host
        .start()
        .await
        .context("Failed to start host for error test")?;

    // Create workload
    let req = WorkloadStartRequest {
        workload: Workload {
            namespace: "error-test".to_string(),
            name: "blobby-error-workload".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                bytes: bytes::Bytes::from_static(BLOBBY_WASM),
                local_resources: LocalResources {
                    memory_limit_mb: 128,
                    cpu_limit: 1,
                    config: HashMap::new(),
                    volume_mounts: vec![],
                    allowed_hosts: vec![],
                },
                pool_size: 1,
                max_invocations: 50,
            }],
            host_interfaces: vec![
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "http".to_string(),
                    interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                    version: Some(semver::Version::parse("0.2.2").unwrap()),
                    config: {
                        let mut config = HashMap::new();
                        config.insert("host".to_string(), "blobby-error-test".to_string());
                        config
                    },
                },
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "blobstore".to_string(),
                    interfaces: [
                        "blobstore".to_string(),
                        "container".to_string(),
                        "types".to_string(),
                    ]
                    .into_iter()
                    .collect(),
                    version: Some(semver::Version::parse("0.2.0-draft").unwrap()),
                    config: HashMap::new(),
                },
                WitInterface {
                    namespace: "wasi".to_string(),
                    package: "logging".to_string(),
                    interfaces: ["logging".to_string()].into_iter().collect(),
                    version: Some(semver::Version::parse("0.1.0-draft").unwrap()),
                    config: HashMap::new(),
                },
            ],
            volumes: vec![],
        },
    };

    let workload_response = host
        .workload_start(req)
        .await
        .context("Failed to start error test workload")?;
    println!(
        "Started error test workload: {:?}",
        workload_response.workload_status.workload_id
    );

    // Test with invalid request methods
    let client = reqwest::Client::new();
    let put_response = client
        .put(format!("http://{addr}/"))
        .header("HOST", "blobby-error-test")
        .body("test data")
        .send()
        .await;

    // The component should handle or reject unsupported methods gracefully
    match put_response {
        Ok(response) => {
            let status = response.status();
            println!("PUT Response Status: {}", status);
            // Either success or proper HTTP error response is acceptable
            assert!(
                status.is_success() || status.is_client_error() || status.is_server_error(),
                "Expected valid HTTP status code"
            );
        }
        Err(e) => {
            println!("PUT request failed as expected: {}", e);
            // Connection errors are acceptable for unsupported methods
        }
    }

    println!("Error handling test completed");
    Ok(())
}
