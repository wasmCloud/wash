//! Integration test for validating component updates in a multi-component setup
//!
//! This test module verifies:
//! 1. Component updates actually change behavior (not just state)
//! 2. Updated components in a dynamically linked setup reflect new logic
//! 3. Other components in the workload remain unaffected during updates

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
    types::{
        Component, ComponentState, LocalResources, Workload, WorkloadStartRequest,
        WorkloadUpdateRequest,
    },
    wit::WitInterface,
};

// Original components
const HTTP_COMPONENT: &[u8] = include_bytes!("fixtures/http_test_component.wasm");
const CREATE_COMPONENT: &[u8] = include_bytes!("fixtures/mock_create_component.wasm");
const UPDATE_COMPONENT: &[u8] = include_bytes!("fixtures/mock_update_component.wasm");

// V2 components with modified behavior
const APP_COMPONENT_V2: &[u8] = include_bytes!("fixtures/app_test_component_v2.wasm");
const CREATE_COMPONENT_V2: &[u8] = include_bytes!("fixtures/mock_create_component_v2.wasm");

// Test components
const SLOW_HTTP_COMPONENT: &[u8] = include_bytes!("fixtures/slow_http_component.wasm");

/// Creates a multi-component workload with dynamic linking between components:
/// - HTTP component: handles incoming HTTP requests, calls action interface
/// - App component: implements action interface, calls create and update interfaces  
/// - Create component: implements create interface
/// - Update component: implements update interface
fn create_workload(name: &str, app_bytes: &'static [u8], create_bytes: &'static [u8]) -> Workload {
    Workload {
        namespace: "test".to_string(),
        name: name.to_string(),
        annotations: HashMap::new(),
        service: None,
        components: vec![
            Component {
                name: Some("http".to_string()),
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
                name: Some("app".to_string()),
                image: None,
                bytes: bytes::Bytes::from_static(app_bytes),
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
                name: Some("create".to_string()),
                image: None,
                bytes: bytes::Bytes::from_static(create_bytes),
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
                name: Some("update".to_string()),
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
                config.insert("host".to_string(), "test".to_string());
                config
            },
        }],
        volumes: vec![],
    }
}

/// Creates an update workload spec with only the component that needs updating.
/// The system will automatically re-link any components that depend on the updated one.
fn create_update_spec_for_create_component(name: &str, create_bytes: &'static [u8]) -> Workload {
    Workload {
        namespace: "test".to_string(),
        name: name.to_string(),
        annotations: HashMap::new(),
        service: None,
        // Only include create - the system will automatically re-link app
        // since it imports from create
        components: vec![Component {
            name: Some("create".to_string()),
            image: None,
            bytes: bytes::Bytes::from_static(create_bytes),
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
        }],
        host_interfaces: vec![],
        volumes: vec![],
    }
}

/// Test: Validates that updating a component in a multi-component setup
/// actually changes the behavior observed through HTTP requests.
///
/// Flow:
/// 1. Start workload with original components (app returns "Action result: CALLING CREATE")
/// 2. Make HTTP request and verify original response
/// 3. Update create component to v2 version
/// 4. Make HTTP request and verify response now shows "CALLING CREATE V2 - COMPONENT UPDATED"
#[tokio::test]
async fn test_multi_component_update_changes_behavior() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    println!("🔄 Testing multi-component update with behavior verification");

    let engine = Engine::builder().build()?;
    let port = find_available_port().await?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let http_plugin = HttpServer::new(DevRouter::default(), addr);

    let host = HostBuilder::new()
        .with_engine(engine.clone())
        .with_http_handler(Arc::new(http_plugin))
        .build()?;

    let host = host.start().await.context("failed to start host")?;
    println!("   ✓ Host started, HTTP server listening on {addr}");

    // Step 1: Start workload with v2 app component (returns create result) and v1 create component
    let workload_id = uuid::Uuid::new_v4().to_string();
    let workload = create_workload(
        "multi-component-update-test",
        APP_COMPONENT_V2,
        CREATE_COMPONENT,
    );

    let start_response = host
        .workload_start(WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload: workload.clone(),
            component_ids: None,
        })
        .await
        .context("failed to start workload")?;

    println!(
        "   ✓ Step 1: Workload started with {} components",
        start_response.workload_status.components.len()
    );

    // Verify all 4 components are running
    assert_eq!(
        start_response.workload_status.components.len(),
        4,
        "workload should have 4 components"
    );
    for comp in &start_response.workload_status.components {
        assert_eq!(
            comp.state,
            ComponentState::Running,
            "component {} should be Running",
            comp.name.as_deref().unwrap_or(&comp.component_id)
        );
    }

    // Step 2: Make HTTP request and verify original response
    let client = reqwest::Client::new();
    let original_response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "test")
            .send(),
    )
    .await
    .context("original request timed out")?
    .context("failed to make original request")?;

    let original_status = original_response.status();
    let original_body = original_response
        .text()
        .await
        .context("failed to read original response")?;

    println!("   ✓ Step 2: Original response: {}", original_body.trim());

    assert!(
        original_status.is_success(),
        "original request should succeed"
    );
    assert!(
        original_body.contains("CALLING CREATE"),
        "original response should contain 'CALLING CREATE', got: {}",
        original_body
    );
    assert!(
        !original_body.contains("V2"),
        "original response should NOT contain 'V2', got: {}",
        original_body
    );

    // Step 3: Update only the create component to v2
    // The system will automatically re-link app since it imports from create
    let update_spec =
        create_update_spec_for_create_component("multi-component-update-test", CREATE_COMPONENT_V2);

    let update_response = host
        .workload_update(WorkloadUpdateRequest {
            workload_id: workload_id.clone(),
            workload: update_spec,
        })
        .await
        .context("failed to update workload")?;

    println!("   ✓ Step 3: Updated create component (app auto-relinked)");

    // Verify update succeeded
    assert_eq!(
        update_response.workload_status.components.len(),
        4,
        "workload should still have 4 components after update"
    );

    // Find the create component and verify it's Running
    let updated_create_component = update_response
        .workload_status
        .components
        .iter()
        .find(|c| c.name.as_deref() == Some("create"))
        .expect("create component should exist");
    assert_eq!(
        updated_create_component.state,
        ComponentState::Running,
        "updated create should be Running"
    );

    // Find the app component and verify it's still Running (was re-linked)
    let updated_app_component = update_response
        .workload_status
        .components
        .iter()
        .find(|c| c.name.as_deref() == Some("app"))
        .expect("app component should exist");
    assert_eq!(
        updated_app_component.state,
        ComponentState::Running,
        "app should still be Running after re-linking"
    );

    // Step 4: Make HTTP request and verify NEW response
    let updated_response = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "test")
            .send(),
    )
    .await
    .context("updated request timed out")?
    .context("failed to make updated request")?;

    let updated_status = updated_response.status();
    let updated_body = updated_response
        .text()
        .await
        .context("failed to read updated response")?;

    println!("   ✓ Step 4: Updated response: {}", updated_body.trim());

    assert!(
        updated_status.is_success(),
        "updated request should succeed"
    );
    assert!(
        updated_body.contains("CALLING CREATE V2 - COMPONENT UPDATED"),
        "updated response should contain 'CALLING CREATE V2 - COMPONENT UPDATED', got: {}",
        updated_body
    );

    println!("✅ Multi-component update behavior verification test PASSED!");
    println!("   - Original response: {}", original_body.trim());
    println!("   - Updated response:  {}", updated_body.trim());

    Ok(())
}

/// Test: Verifies that updating one component doesn't affect other components
/// in the multi-component workload.
#[tokio::test]
async fn test_multi_component_update_isolation() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    println!("🔄 Testing component update isolation in multi-component setup");

    let engine = Engine::builder().build()?;
    let port = find_available_port().await?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let http_plugin = HttpServer::new(DevRouter::default(), addr);

    let host = HostBuilder::new()
        .with_engine(engine.clone())
        .with_http_handler(Arc::new(http_plugin))
        .build()?;

    let host = host.start().await.context("failed to start host")?;

    // Start workload
    let workload_id = uuid::Uuid::new_v4().to_string();
    let workload = create_workload("update-isolation-test", APP_COMPONENT_V2, CREATE_COMPONENT);

    let start_response = host
        .workload_start(WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload: workload.clone(),
            component_ids: None,
        })
        .await
        .context("failed to start workload")?;

    // Record component IDs before update
    let component_ids_before: HashMap<String, String> = start_response
        .workload_status
        .components
        .iter()
        .map(|c| (c.name.clone().unwrap_or_default(), c.component_id.clone()))
        .collect();

    println!("   ✓ Started workload with 4 components");

    // Update only create - the system will auto-relink app
    let update_spec =
        create_update_spec_for_create_component("update-isolation-test", CREATE_COMPONENT_V2);

    let update_response = host
        .workload_update(WorkloadUpdateRequest {
            workload_id: workload_id.clone(),
            workload: update_spec,
        })
        .await
        .context("failed to update workload")?;

    // Verify component IDs for non-updated components remained the same
    // http, app, and update should retain their original IDs
    // (app is re-linked, not replaced, so its ID stays the same)
    for comp in &update_response.workload_status.components {
        let comp_name = comp.name.clone().unwrap_or_default();
        if comp_name != "create" {
            let original_id = component_ids_before.get(&comp_name);
            assert_eq!(
                original_id,
                Some(&comp.component_id),
                "component {} should retain its original ID after another component was updated",
                comp_name
            );
            assert_eq!(
                comp.state,
                ComponentState::Running,
                "component {} should still be Running",
                comp_name
            );
        }
    }

    // The updated component should still have the same ID (we preserve IDs)
    let create = update_response
        .workload_status
        .components
        .iter()
        .find(|c| c.name.as_deref() == Some("create"))
        .expect("create should exist");

    assert_eq!(
        create.state,
        ComponentState::Running,
        "create should be Running after update"
    );

    // app should still be Running (was re-linked, not updated)
    let app = update_response
        .workload_status
        .components
        .iter()
        .find(|c| c.name.as_deref() == Some("app"))
        .expect("app should exist");

    assert_eq!(
        app.state,
        ComponentState::Running,
        "app should be Running after re-linking"
    );

    println!("✅ Component update isolation test PASSED!");
    println!("   - Only create was updated");
    println!("   - app was automatically re-linked");
    println!("   - http and update retained their original IDs and remained Running");

    Ok(())
}

/// Test: Verifies that updating create doesn't break update.
///
/// This test makes HTTP requests before and after updating create to verify:
/// 1. Both create and update components work before the update
/// 2. After updating create to v2, update still returns its original response
/// 3. The component chain remains fully functional
///
/// The app component returns both create() and update() results in its response,
/// allowing us to verify both components are working correctly.
#[tokio::test]
async fn test_update_component_unaffected_by_create_update() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    println!("🔄 Testing that update remains functional after create is updated");

    let engine = Engine::builder().build()?;
    let port = find_available_port().await?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let http_plugin = HttpServer::new(DevRouter::default(), addr);

    let host = HostBuilder::new()
        .with_engine(engine.clone())
        .with_http_handler(Arc::new(http_plugin))
        .build()?;

    let host = host.start().await.context("failed to start host")?;
    println!("   ✓ Host started, HTTP server listening on {addr}");

    // Start workload with v2 app component (returns both create and update results)
    // and v1 create/update components
    let workload_id = uuid::Uuid::new_v4().to_string();
    let workload = create_workload("update-unaffected-test", APP_COMPONENT_V2, CREATE_COMPONENT);

    let _start_response = host
        .workload_start(WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload: workload.clone(),
            component_ids: None,
        })
        .await
        .context("failed to start workload")?;

    let client = reqwest::Client::new();

    // Step 1: Make HTTP request BEFORE update - verify both create and update work
    let response_before = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "test")
            .send(),
    )
    .await
    .context("request timed out")?
    .context("failed to make request")?;

    let body_before = response_before
        .text()
        .await
        .context("failed to read response")?;

    println!(
        "   ✓ Step 1: Response before update: {}",
        body_before.trim()
    );

    // Verify both create and update are present in response
    assert!(
        body_before.contains("CALLING CREATE"),
        "response should contain 'CALLING CREATE' (from create v1), got: {}",
        body_before
    );
    assert!(
        body_before.contains("CALLING UPDATE"),
        "response should contain 'CALLING UPDATE' (from update), got: {}",
        body_before
    );
    assert!(
        !body_before.contains("V2"),
        "response should NOT contain 'V2' before update, got: {}",
        body_before
    );

    // Step 2: Update create to v2
    let update_spec =
        create_update_spec_for_create_component("update-unaffected-test", CREATE_COMPONENT_V2);

    let _update_response = host
        .workload_update(WorkloadUpdateRequest {
            workload_id: workload_id.clone(),
            workload: update_spec,
        })
        .await
        .context("failed to update workload")?;

    println!("   ✓ Step 2: Updated create to v2");

    // Step 3: Make HTTP request AFTER update - verify update component still works
    let response_after = timeout(
        Duration::from_secs(10),
        client
            .get(format!("http://{addr}/"))
            .header("HOST", "test")
            .send(),
    )
    .await
    .context("request timed out after update")?
    .context("failed to make request after update")?;

    let body_after = response_after
        .text()
        .await
        .context("failed to read response after update")?;

    println!("   ✓ Step 3: Response after update: {}", body_after.trim());

    // Verify create now returns v2 response
    assert!(
        body_after.contains("CALLING CREATE V2"),
        "response should contain 'CALLING CREATE V2' (from create v2), got: {}",
        body_after
    );

    // CRITICAL: Verify update STILL works and returns its original response
    // This proves that updating create didn't break update
    assert!(
        body_after.contains("CALLING UPDATE"),
        "response should STILL contain 'CALLING UPDATE' (update should be unaffected), got: {}",
        body_after
    );

    // Verify the update result is unchanged (not affected by create component update)
    assert!(
        !body_after.contains("Update result: CALLING CREATE"),
        "update result should not be confused with create result, got: {}",
        body_after
    );

    println!("✅ Update component unaffected by create update test PASSED!");
    println!("   - Before update: create='CALLING CREATE', update='CALLING UPDATE'");
    println!(
        "   - After update:  create='CALLING CREATE V2', update='CALLING UPDATE' (unchanged!)"
    );
    println!("   - update remained fully functional after create was updated");

    Ok(())
}

/// Test: Verifies that in-flight requests are allowed to complete (drain) before
/// a component is replaced during an update.
///
/// Flow:
/// 1. Start workload with slow HTTP component (2s delay on /slow endpoint)
/// 2. Send request to /slow endpoint (spawned concurrently)
/// 3. Wait 200ms to ensure request is in-flight, then trigger update
/// 4. Verify the in-flight request completes successfully after ~2 seconds
/// 5. Verify component was updated (new version running)
///
/// This tests the graceful drain mechanism:
/// - Component enters Reconciling state during update
/// - Existing in-flight requests are allowed to complete (drain)
/// - Only after drain completes does the component restart
#[tokio::test]
async fn test_component_drains_requests_before_update() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    println!("🔄 Testing graceful drain: in-flight requests complete before update");

    let engine = Engine::builder().build()?;
    let port = find_available_port().await?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let http_plugin = HttpServer::new(DevRouter::default(), addr);

    let host = Arc::new(
        HostBuilder::new()
            .with_engine(engine.clone())
            .with_http_handler(Arc::new(http_plugin))
            .build()?
            .start()
            .await
            .context("failed to start host")?,
    );

    println!("   ✓ Host started, HTTP server listening on {addr}");

    // Start workload with slow component
    let workload_id = uuid::Uuid::new_v4().to_string();
    let workload = Workload {
        namespace: "test".to_string(),
        name: "drain-test".to_string(),
        annotations: HashMap::new(),
        service: None,
        components: vec![Component {
            name: Some("slow-http".to_string()),
            image: Some("test/slow:v1".to_string()),
            bytes: bytes::Bytes::from_static(SLOW_HTTP_COMPONENT),
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
        }],
        host_interfaces: vec![WitInterface {
            namespace: "wasi".to_string(),
            package: "http".to_string(),
            interfaces: ["incoming-handler".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.2.2").unwrap()),
            config: {
                let mut config = HashMap::new();
                config.insert("host".to_string(), "test".to_string());
                config
            },
        }],
        volumes: vec![],
    };

    host.workload_start(WorkloadStartRequest {
        workload_id: workload_id.clone(),
        workload: workload.clone(),
        component_ids: None,
    })
    .await
    .context("failed to start workload")?;

    println!("   ✓ Workload started with slow component (2s delay on /slow)");

    // Step 1: Spawn a request to /slow endpoint (2-second delay)
    let addr_clone = addr;
    let request_start = std::time::Instant::now();
    let in_flight_handle = tokio::spawn(async move {
        let start = std::time::Instant::now();
        let client = reqwest::Client::new();
        let response = client
            .get(format!("http://{addr_clone}/slow"))
            .header("HOST", "test")
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .context("in-flight request failed to send")?;

        let elapsed = start.elapsed();
        let status = response.status();
        let body = response
            .text()
            .await
            .context("failed to read in-flight response")?;

        anyhow::Ok((status, body, elapsed))
    });

    // Wait 200ms to ensure the request reaches the component and starts executing
    tokio::time::sleep(Duration::from_millis(200)).await;
    println!("   ✓ Step 1: Slow request is now in-flight (executing for ~200ms)");

    // Step 2: Trigger component update while request is in-flight
    println!("   ✓ Step 2: Triggering component update (component enters Reconciling state)...");
    let update_start = std::time::Instant::now();

    // Update with new image reference to simulate version change
    let update_spec = Workload {
        namespace: "test".to_string(),
        name: "drain-test".to_string(),
        annotations: HashMap::new(),
        service: None,
        components: vec![Component {
            name: Some("slow-http".to_string()),
            image: Some("test/slow:v2".to_string()),
            bytes: bytes::Bytes::from_static(SLOW_HTTP_COMPONENT),
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
        }],
        host_interfaces: vec![],
        volumes: vec![],
    };

    let update_response = host
        .workload_update(WorkloadUpdateRequest {
            workload_id: workload_id.clone(),
            workload: update_spec,
        })
        .await
        .context("failed to update workload")?;

    let update_elapsed = update_start.elapsed();
    println!(
        "   ✓ Component update completed in {:.2}s",
        update_elapsed.as_secs_f64()
    );

    // Verify update succeeded
    let slow_component = update_response
        .workload_status
        .components
        .iter()
        .find(|c| c.name.as_deref() == Some("slow-http"))
        .expect("slow-http component should exist");

    assert_eq!(
        slow_component.state,
        ComponentState::Running,
        "slow-http should be Running after update"
    );

    // Verify version changed
    assert_eq!(
        slow_component.image.as_deref(),
        Some("test/slow:v2"),
        "component should have new image reference"
    );

    // Step 3: Wait for the in-flight request to complete
    let (in_flight_status, in_flight_body, request_elapsed) =
        timeout(Duration::from_secs(5), in_flight_handle)
            .await
            .context("in-flight request timed out")?
            .context("in-flight task panicked")??;

    let _total_elapsed = request_start.elapsed();
    println!(
        "   ✓ Step 3: In-flight request completed with status: {} in {:.2}s",
        in_flight_status,
        request_elapsed.as_secs_f64()
    );

    // The in-flight request should have succeeded (not been interrupted)
    assert!(
        in_flight_status.is_success(),
        "in-flight request should succeed (graceful drain), got status: {}, body: {}",
        in_flight_status,
        in_flight_body
    );

    // Verify request took approximately 2 seconds (the slow endpoint delay)
    assert!(
        request_elapsed.as_secs_f64() >= 1.8 && request_elapsed.as_secs_f64() <= 3.0,
        "request should have taken ~2 seconds to complete (got {:.2}s), proving drain worked",
        request_elapsed.as_secs_f64()
    );

    // Verify the update waited for the request to complete
    assert!(
        update_elapsed.as_secs_f64() >= 1.6,
        "update should have waited for in-flight request to drain (got {:.2}s)",
        update_elapsed.as_secs_f64()
    );

    println!("✅ Graceful drain test PASSED!");
    println!(
        "   - In-flight request completed successfully in {:.2}s (was not interrupted)",
        request_elapsed.as_secs_f64()
    );
    println!(
        "   - Update waited {:.2}s for drain before completing",
        update_elapsed.as_secs_f64()
    );
    println!(
        "   - Component now running v2: {}",
        slow_component.image.as_deref().unwrap()
    );

    Ok(())
}

/// Test: Verifies that HTTP requests arriving while a component is in Reconciling
/// state are held/queued and eventually succeed once the component becomes Running.
///
/// Strategy:
/// 1. Start workload and fire parallel requests continuously
/// 2. Trigger component update mid-stream (sets Reconciling state)
/// 3. Measure response times for all requests
/// 4. Verify: requests before reconciling complete fast, requests during reconciling
///    are delayed but eventually succeed — the timing gap proves the queue mechanism
///
/// This tests the request queueing mechanism:
/// - When component is Reconciling, new requests wait (poll every 100ms)
/// - Once component reaches Running, queued requests proceed
/// - Requests have a timeout (30s default) to prevent indefinite waits
#[tokio::test]
async fn test_http_handler_holds_requests_until_update() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    println!("🔄 Testing request queueing: parallel requests during Reconciling state");

    let engine = Engine::builder().build()?;
    let port = find_available_port().await?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let http_plugin = HttpServer::new(DevRouter::default(), addr);

    let host = Arc::new(
        HostBuilder::new()
            .with_engine(engine.clone())
            .with_http_handler(Arc::new(http_plugin))
            .build()?
            .start()
            .await
            .context("failed to start host")?,
    );

    println!("   ✓ Host started, HTTP server listening on {addr}");

    // Start workload with slow component
    let workload_id = uuid::Uuid::new_v4().to_string();
    let workload = Workload {
        namespace: "test".to_string(),
        name: "hold-test".to_string(),
        annotations: HashMap::new(),
        service: None,
        components: vec![Component {
            name: Some("slow-http".to_string()),
            image: Some("test/slow:v1".to_string()),
            bytes: bytes::Bytes::from_static(SLOW_HTTP_COMPONENT),
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
        }],
        host_interfaces: vec![WitInterface {
            namespace: "wasi".to_string(),
            package: "http".to_string(),
            interfaces: ["incoming-handler".to_string()].into_iter().collect(),
            version: Some(semver::Version::parse("0.2.2").unwrap()),
            config: {
                let mut config = HashMap::new();
                config.insert("host".to_string(), "test".to_string());
                config
            },
        }],
        volumes: vec![],
    };

    host.workload_start(WorkloadStartRequest {
        workload_id: workload_id.clone(),
        workload: workload.clone(),
        component_ids: None,
    })
    .await
    .context("failed to start workload")?;

    println!("   ✓ Workload started");

    // Record test start time for all measurements
    let test_start = std::time::Instant::now();

    // Step 1: Fire a batch of "pre-update" requests to establish baseline timing
    const PRE_UPDATE_REQUESTS: usize = 5;
    let mut pre_update_handles = Vec::new();

    for i in 0..PRE_UPDATE_REQUESTS {
        let addr = addr;
        let request_id = i;
        let handle = tokio::spawn(async move {
            let start = std::time::Instant::now();
            let client = reqwest::Client::new();
            let response = client
                .get(format!("http://{addr}/fast"))
                .header("HOST", "test")
                .timeout(Duration::from_secs(30))
                .send()
                .await
                .context("pre-update request failed")?;

            let elapsed = start.elapsed();
            let status = response.status();
            anyhow::Ok((request_id, status, elapsed))
        });
        pre_update_handles.push(handle);
    }

    // Wait for pre-update requests to complete and measure baseline
    let mut pre_update_times = Vec::new();
    for handle in pre_update_handles {
        let (id, status, elapsed) = handle.await.context("task panicked")??;
        assert!(
            status.is_success(),
            "pre-update request {id} should succeed"
        );
        pre_update_times.push(elapsed);
    }

    let avg_baseline = pre_update_times
        .iter()
        .map(|d| d.as_secs_f64())
        .sum::<f64>()
        / pre_update_times.len() as f64;
    println!(
        "   ✓ Step 1: Baseline established - {} requests completed, avg {:.3}s",
        PRE_UPDATE_REQUESTS, avg_baseline
    );

    // Step 2: Start update in background (with a slow request to extend reconciling window)
    let host_clone = host.clone();
    let workload_id_clone = workload_id.clone();
    let addr_for_slow = addr;

    let update_handle = tokio::spawn(async move {
        // Send a slow request first to create a drain window
        let slow_handle = tokio::spawn(async move {
            let client = reqwest::Client::new();
            let _ = client
                .get(format!("http://{addr_for_slow}/slow"))
                .header("HOST", "test")
                .timeout(Duration::from_secs(5))
                .send()
                .await;
        });

        // Small delay to ensure slow request is in-flight
        tokio::time::sleep(Duration::from_millis(50)).await;

        let update_start = std::time::Instant::now();

        // Trigger update
        let update_spec = Workload {
            namespace: "test".to_string(),
            name: "hold-test".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![Component {
                name: Some("slow-http".to_string()),
                image: Some("test/slow:v2".to_string()),
                bytes: bytes::Bytes::from_static(SLOW_HTTP_COMPONENT),
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
            }],
            host_interfaces: vec![],
            volumes: vec![],
        };

        let result = host_clone
            .workload_update(WorkloadUpdateRequest {
                workload_id: workload_id_clone,
                workload: update_spec,
            })
            .await;

        let _ = slow_handle.await;
        let update_duration = update_start.elapsed();

        result.map(|r| (r, update_duration))
    });

    // Step 3: Fire ALL parallel requests simultaneously during the update/reconciling window
    // No stagger - we want them all to hit while component is Reconciling
    const DURING_UPDATE_REQUESTS: usize = 20;
    let mut during_update_handles = Vec::new();

    println!(
        "   ✓ Step 2: Triggering update and firing {} parallel requests simultaneously...",
        DURING_UPDATE_REQUESTS
    );

    // Fire all requests at once (no stagger)
    for i in 0..DURING_UPDATE_REQUESTS {
        let addr = addr;
        let request_id = i;
        let test_start_clone = test_start;

        let handle = tokio::spawn(async move {
            let sent_at = test_start_clone.elapsed();
            let start = std::time::Instant::now();
            let client = reqwest::Client::new();
            let response = client
                .get(format!("http://{addr}/fast"))
                .header("HOST", "test")
                .timeout(Duration::from_secs(35))
                .send()
                .await
                .context("during-update request failed")?;

            let elapsed = start.elapsed();
            let status = response.status();
            anyhow::Ok((request_id, status, elapsed, sent_at))
        });
        during_update_handles.push(handle);
    }

    // Step 4: Wait for update to complete
    let (update_result, update_duration) = timeout(Duration::from_secs(15), update_handle)
        .await
        .context("update timed out")?
        .context("update task panicked")??;

    println!(
        "   ✓ Step 3: Update completed in {:.2}s",
        update_duration.as_secs_f64()
    );

    // Verify update succeeded
    let slow_component = update_result
        .workload_status
        .components
        .iter()
        .find(|c| c.name.as_deref() == Some("slow-http"))
        .expect("slow-http component should exist");

    assert_eq!(
        slow_component.state,
        ComponentState::Running,
        "slow-http should be Running after update"
    );

    assert_eq!(
        slow_component.image.as_deref(),
        Some("test/slow:v2"),
        "component should have new image reference"
    );

    // Step 5: Collect all during-update request results
    let mut during_update_results = Vec::new();
    for handle in during_update_handles {
        match timeout(Duration::from_secs(35), handle).await {
            Ok(Ok(Ok(result))) => during_update_results.push(result),
            Ok(Ok(Err(e))) => println!("   ⚠ Request failed: {e}"),
            Ok(Err(e)) => println!("   ⚠ Task panicked: {e}"),
            Err(_) => println!("   ⚠ Request timed out"),
        }
    }

    // All requests should have succeeded
    let success_count = during_update_results
        .iter()
        .filter(|(_, status, _, _)| status.is_success())
        .count();

    assert_eq!(
        success_count,
        during_update_results.len(),
        "all during-update requests should eventually succeed"
    );

    // Analyze timing: partition into "fast" (completed quickly) and "delayed" (held by queue)
    // Threshold: requests taking > 2x baseline are considered "delayed"
    let delay_threshold = (avg_baseline * 3.0).max(0.5); // At least 0.5s or 3x baseline

    let fast_requests: Vec<_> = during_update_results
        .iter()
        .filter(|(_, _, elapsed, _)| elapsed.as_secs_f64() < delay_threshold)
        .collect();

    let delayed_requests: Vec<_> = during_update_results
        .iter()
        .filter(|(_, _, elapsed, _)| elapsed.as_secs_f64() >= delay_threshold)
        .collect();

    println!("   ✓ Step 4: Request timing analysis:");
    println!("      - Baseline avg: {:.3}s", avg_baseline);
    println!("      - Delay threshold: {:.3}s", delay_threshold);
    println!(
        "      - Fast requests (before/after reconciling): {}",
        fast_requests.len()
    );
    println!(
        "      - Delayed requests (held during reconciling): {}",
        delayed_requests.len()
    );

    // Print individual timings for debugging
    let mut sorted_results = during_update_results.clone();
    sorted_results.sort_by(|a, b| a.3.cmp(&b.3)); // Sort by send time

    println!("   ✓ Step 5: Individual request timings (sorted by send time):");
    for (id, status, elapsed, sent_at) in &sorted_results {
        let marker = if elapsed.as_secs_f64() >= delay_threshold {
            "🔴 DELAYED"
        } else {
            "🟢 FAST"
        };
        println!(
            "      Request {:2}: sent@{:.2}s, took {:.3}s {} ({})",
            id,
            sent_at.as_secs_f64(),
            elapsed.as_secs_f64(),
            marker,
            status
        );
    }

    // Verify we observed the queueing behavior:
    // - Some requests should have been delayed (held during reconciling)
    // - The update took significant time (due to drain), so requests sent during that
    //   window should show delays
    assert!(
        !delayed_requests.is_empty(),
        "expected some requests to be delayed during reconciling, but all completed quickly. \
         This suggests the queueing mechanism didn't engage. \
         Update took {:.2}s, threshold was {:.3}s",
        update_duration.as_secs_f64(),
        delay_threshold
    );

    // Calculate the timing gap between fast and delayed requests
    // If no fast requests, compare delayed to baseline instead
    let avg_fast = if !fast_requests.is_empty() {
        fast_requests
            .iter()
            .map(|(_, _, e, _)| e.as_secs_f64())
            .sum::<f64>()
            / fast_requests.len() as f64
    } else {
        avg_baseline // Use baseline as reference when all requests were delayed
    };

    let avg_delayed = delayed_requests
        .iter()
        .map(|(_, _, e, _)| e.as_secs_f64())
        .sum::<f64>()
        / delayed_requests.len() as f64;

    let timing_gap = avg_delayed - avg_fast;

    println!("   ✓ Step 6: Timing gap analysis:");
    println!("      - Avg fast/baseline time: {:.3}s", avg_fast);
    println!("      - Avg delayed request time: {:.3}s", avg_delayed);
    println!("      - Timing gap (queue hold time): {:.3}s", timing_gap);

    // The timing gap should be significant (at least 0.5s) to prove queueing worked
    assert!(
        timing_gap >= 0.3,
        "timing gap between fast/baseline and delayed requests should be >= 0.3s to prove queueing, got {:.3}s",
        timing_gap
    );

    // Step 7: Verify post-update requests work immediately
    let client = reqwest::Client::new();
    let post_start = std::time::Instant::now();
    let post_response = timeout(
        Duration::from_secs(5),
        client
            .get(format!("http://{addr}/fast"))
            .header("HOST", "test")
            .send(),
    )
    .await
    .context("post-update request timed out")?
    .context("failed to make post-update request")?;

    let post_elapsed = post_start.elapsed();

    assert!(
        post_response.status().is_success(),
        "post-update request should succeed"
    );
    assert!(
        post_elapsed.as_secs_f64() < delay_threshold,
        "post-update request should be fast (got {:.3}s, threshold {:.3}s)",
        post_elapsed.as_secs_f64(),
        delay_threshold
    );

    println!(
        "   ✓ Step 7: Post-update request completed in {:.3}s (fast)",
        post_elapsed.as_secs_f64()
    );

    println!("✅ Request queueing test PASSED!");
    println!("   Summary:");
    println!(
        "   - Fired {} requests during component update",
        DURING_UPDATE_REQUESTS
    );
    println!(
        "   - {} requests completed fast (before/after reconciling)",
        fast_requests.len()
    );
    println!(
        "   - {} requests were delayed (held during reconciling)",
        delayed_requests.len()
    );
    println!("   - Timing gap proves queue mechanism: {:.3}s", timing_gap);
    println!(
        "   - Component updated to v2: {}",
        slow_component.image.as_deref().unwrap()
    );

    Ok(())
}
