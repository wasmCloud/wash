//! Integration test for blocks app component to work on dynamic linking issue

use anyhow::{Context, Result};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::time::timeout;

mod common;
use common::find_available_port;

use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, HttpServer},
    },
    types::{Component, LocalResources, Workload, WorkloadStartRequest},
    wit::WitInterface,
};

const HTTP_COMPONENT: &[u8] = include_bytes!("fixtures/http_test_component.wasm");
const APP_COMPONENT: &[u8] = include_bytes!("fixtures/app_test_component.wasm");
const CREATE_COMPONENT: &[u8] = include_bytes!("fixtures/mock_create_component.wasm");
const UPDATE_COMPONENT: &[u8] = include_bytes!("fixtures/mock_update_component.wasm");

#[tokio::test]
async fn test_dynamic_linking() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let engine = Engine::builder().build()?;

    let port = find_available_port().await?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let http_handler = DevRouter::default();
    let http_plugin = HttpServer::new(http_handler, addr);

    let host = HostBuilder::new()
        .with_engine(engine.clone())
        .with_http_handler(Arc::new(http_plugin))
        .build()?;

    let host = host.start().await.context("Failed to start host")?;
    println!("Host started, HTTP server listening on {addr}");

    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
        workload: Workload {
            namespace: "test".to_string(),
            name: "dynamic-linking".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![
                Component {
                    name: Some("http-component".to_string()),
                    image: None,
                    bytes: bytes::Bytes::from_static(HTTP_COMPONENT),
                    local_resources: LocalResources {
                        memory_limit_mb: 256,
                        cpu_limit: 1,
                        config: HashMap::new(),
                        environment: HashMap::new(),
                        volume_mounts: vec![],
                        allowed_hosts: vec![],
                    },
                    pool_size: 1,
                    max_invocations: 100,
                },
                Component {
                    name: Some("App-component".to_string()),
                    image: None,
                    bytes: bytes::Bytes::from_static(APP_COMPONENT),
                    local_resources: LocalResources {
                        memory_limit_mb: 256,
                        cpu_limit: 1,
                        config: HashMap::new(),
                        environment: HashMap::new(),
                        volume_mounts: vec![],
                        allowed_hosts: vec![],
                    },
                    pool_size: 1,
                    max_invocations: 100,
                },
                Component {
                    name: Some("create-component".to_string()),
                    image: None,
                    bytes: bytes::Bytes::from_static(CREATE_COMPONENT),
                    local_resources: LocalResources {
                        memory_limit_mb: 256,
                        cpu_limit: 1,
                        config: HashMap::new(),
                        environment: HashMap::new(),
                        volume_mounts: vec![],
                        allowed_hosts: vec![],
                    },
                    pool_size: 1,
                    max_invocations: 100,
                },
                Component {
                    name: Some("update-component".to_string()),
                    image: None,
                    bytes: bytes::Bytes::from_static(UPDATE_COMPONENT),
                    local_resources: LocalResources {
                        memory_limit_mb: 256,
                        cpu_limit: 1,
                        config: HashMap::new(),
                        environment: HashMap::new(),
                        volume_mounts: vec![],
                        allowed_hosts: vec![],
                    },
                    pool_size: 1,
                    max_invocations: 100,
                },
            ],
            host_interfaces: vec![WitInterface {
                namespace: "wasi".to_string(),
                package: "http".to_string(),
                interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                version: Some(semver::Version::parse("0.2.2").unwrap()),
                config: {
                    let mut config = HashMap::new();
                    config.insert("host".to_string(), "foo".to_string());
                    config
                },
            }],
            volumes: vec![],
        },
        component_ids: None,
    };

    let workload_response = host
        .workload_start(req)
        .await
        .context("Failed to start workload")?;

    println!("\n╔═══════════════════════════════════════════════════════════════════════╗");
    println!("║                         🚀 WORKLOAD DEPLOYED                          ║");
    println!("╠═══════════════════════════════════════════════════════════════════════╣");
    println!(
        "║ Workload ID: {:51} ║",
        workload_response.workload_status.workload_id
    );

    let client = reqwest::Client::new();

    let first_response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "foo")
            .send(),
    )
    .await
    .context("Request timedout")?
    .context("Failed to make request")?;

    let status = first_response.status();
    println!("Response Status: {}", status);

    let response_text = first_response
        .text()
        .await
        .context("Failed to read response body")?;
    println!("Response Body: {}", response_text.trim());

    assert!(
        status.is_success(),
        "Request failed with status {}: {}",
        status,
        response_text.trim()
    );

    assert_eq!(response_text.trim(), "Ran action",);

    Ok(())
}
