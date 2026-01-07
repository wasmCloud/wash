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
