//! Integration tests for component-specific lifecycle management
//!
//! This test module verifies:
//! 1. Component discovery via workload_status
//! 2. Component-specific stop (keeps components in memory with Stopped state)
//! 3. Component-specific start/restart (restarts stopped components)
//! 4. State transitions: Starting → Reconciling → Running → Stopped
//! 5. workload_update functionality

use anyhow::{Context, Result};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::time::timeout;

mod common;
use common::find_available_port;

use wash::plugin::PluginManager;
use wash_runtime::{
    engine::Engine,
    host::{
        HostApi, HostBuilder,
        http::{DevRouter, HttpServer},
    },
    plugin::{wasi_config::WasiConfig, wasi_keyvalue::WasiKeyvalue, wasi_logging::WasiLogging},
    types::{
        Component, ComponentState, LocalResources, Workload, WorkloadStartRequest, WorkloadState,
        WorkloadStatusRequest, WorkloadStopRequest, WorkloadUpdateRequest,
    },
    wit::WitInterface,
};

const HTTP_HELLO_WASM: &[u8] = include_bytes!("fixtures/http_hello_world_rust.wasm");
const HTTP_COUNTER_WASM: &[u8] = include_bytes!("fixtures/http_counter.wasm");
const BLOBSTORE_FS_WASM: &[u8] = include_bytes!("fixtures/blobstore_filesystem.wasm");

/// Helper to create a basic test workload with a single HTTP component
fn create_test_workload(name: &str) -> Workload {
    Workload {
        namespace: "test".to_string(),
        name: name.to_string(),
        annotations: HashMap::new(),
        service: None,
        components: vec![Component {
            name: Some("main-component".to_string()),
            image: None,
            bytes: bytes::Bytes::from_static(HTTP_HELLO_WASM),
            local_resources: LocalResources {
                memory_limit_mb: 128,
                cpu_limit: 1,
                config: HashMap::new(),
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
                version: None,
                config: {
                    let mut config = HashMap::new();
                    config.insert("host".to_string(), "test".to_string());
                    config
                },
            },
            WitInterface {
                namespace: "wasi".to_string(),
                package: "logging".to_string(),
                interfaces: ["logging".to_string()].into_iter().collect(),
                version: Some(semver::Version::parse("0.1.0-draft").unwrap()),
                config: HashMap::new(),
            },
            WitInterface {
                namespace: "wasi".to_string(),
                package: "config".to_string(),
                interfaces: ["store".to_string()].into_iter().collect(),
                version: Some(semver::Version::parse("0.2.0-rc.1").unwrap()),
                config: HashMap::new(),
            },
        ],
        volumes: vec![],
    }
}

/// Helper to create a multi-component workload with different interfaces
/// HTTP counter exports wasi:http/incoming-handler, blobstore-fs exports wasi:blobstore
fn create_multi_component_workload(name: &str) -> Workload {
    Workload {
        namespace: "test".to_string(),
        name: name.to_string(),
        annotations: HashMap::new(),
        service: None,
        components: vec![
            // Component 1: HTTP counter
            Component {
                name: Some("http-counter".to_string()),
                image: None,
                bytes: bytes::Bytes::from_static(HTTP_COUNTER_WASM),
                local_resources: LocalResources {
                    memory_limit_mb: 256,
                    cpu_limit: 2,
                    config: HashMap::new(),
                    environment: HashMap::new(),
                    volume_mounts: vec![],
                    allowed_hosts: vec![],
                },
                pool_size: 1,
                max_invocations: 100,
            },
            // Component 2: Blobstore filesystem
            Component {
                name: Some("blobstore-fs".to_string()),
                image: None,
                bytes: bytes::Bytes::from_static(BLOBSTORE_FS_WASM),
                local_resources: LocalResources {
                    memory_limit_mb: 128,
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
        host_interfaces: vec![
            WitInterface {
                namespace: "wasi".to_string(),
                package: "http".to_string(),
                interfaces: ["incoming-handler".to_string()].into_iter().collect(),
                version: None,
                config: {
                    let mut config = HashMap::new();
                    config.insert("host".to_string(), "test".to_string());
                    config
                },
            },
            WitInterface {
                namespace: "wasi".to_string(),
                package: "keyvalue".to_string(),
                interfaces: ["store".to_string(), "atomics".to_string()]
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
            WitInterface {
                namespace: "wasi".to_string(),
                package: "config".to_string(),
                interfaces: ["store".to_string()].into_iter().collect(),
                version: Some(semver::Version::parse("0.2.0-rc.1").unwrap()),
                config: HashMap::new(),
            },
            WitInterface {
                namespace: "wasmcloud".to_string(),
                package: "wash".to_string(),
                interfaces: ["types".to_string()].into_iter().collect(),
                version: Some(semver::Version::parse("0.0.2").unwrap()),
                config: HashMap::new(),
            },
        ],
        volumes: vec![],
    }
}

/// Helper to create a host with basic plugins
async fn create_test_host() -> Result<(impl HostApi, u16)> {
    let engine = Engine::builder().build()?;

    let port = find_available_port().await?;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let http_plugin = HttpServer::new(DevRouter::default(), addr);
    let logging_plugin = WasiLogging {};
    let config_plugin = WasiConfig::default();
    let keyvalue_plugin = WasiKeyvalue::new();
    let plugin_manager = PluginManager::default();

    let host = HostBuilder::new()
        .with_engine(engine)
        .with_http_handler(Arc::new(http_plugin))
        .with_plugin(Arc::new(logging_plugin))?
        .with_plugin(Arc::new(config_plugin))?
        .with_plugin(Arc::new(keyvalue_plugin))?
        .with_plugin(Arc::new(plugin_manager))?
        .build()?;

    let host = host.start().await.context("failed to start host")?;
    Ok((host, port))
}

/// Test 1: Component discovery via workload_status
/// Verifies that workload_status returns component information with IDs
#[tokio::test]
async fn test_component_discovery_via_status() -> Result<()> {
    let (host, _port) = create_test_host().await?;

    let workload_id = uuid::Uuid::new_v4().to_string();
    let workload = create_test_workload("component-discovery-test");

    // Start the workload
    let start_response = host
        .workload_start(WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload,
            component_ids: None,
        })
        .await
        .context("failed to start workload")?;

    // Verify workload started
    assert_eq!(
        start_response.workload_status.workload_state,
        WorkloadState::Running
    );

    // Verify components are returned with IDs
    assert!(
        !start_response.workload_status.components.is_empty(),
        "workload should have components"
    );

    // Get status and verify component discovery
    let status_response = host
        .workload_status(WorkloadStatusRequest {
            workload_id: workload_id.clone(),
        })
        .await
        .context("failed to get workload status")?;

    // Verify status has components
    assert!(
        !status_response.workload_status.components.is_empty(),
        "workload status should include components"
    );

    // Verify each component has an ID and is in Running state
    for component in &status_response.workload_status.components {
        assert!(
            !component.component_id.is_empty(),
            "component should have an ID"
        );
        assert_eq!(
            component.state,
            ComponentState::Running,
            "component should be in Running state"
        );
    }

    println!("✅ Component discovery test passed");
    println!(
        "   Found {} component(s)",
        status_response.workload_status.components.len()
    );
    for comp in &status_response.workload_status.components {
        println!("   - {} (state: {:?})", comp.component_id, comp.state);
    }

    Ok(())
}

/// Test 2: Component-specific stop
/// Verifies that stopping specific components keeps them in memory with Stopped state
#[tokio::test]
async fn test_component_specific_stop() -> Result<()> {
    let (host, _port) = create_test_host().await?;

    let workload_id = uuid::Uuid::new_v4().to_string();
    let workload = create_test_workload("component-stop-test");

    // Start the workload
    let start_response = host
        .workload_start(WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload,
            component_ids: None,
        })
        .await
        .context("failed to start workload")?;

    // Verify we have a component
    assert!(
        !start_response.workload_status.components.is_empty(),
        "workload should have a component"
    );

    // Get the component ID
    let component_to_stop = start_response.workload_status.components[0]
        .component_id
        .clone();

    // Stop the component
    let stop_response = host
        .workload_stop(WorkloadStopRequest {
            workload_id: workload_id.clone(),
            component_ids: Some(vec![component_to_stop.clone()]),
        })
        .await
        .context("failed to stop specific component")?;

    // Verify workload is still running (not fully stopped)
    assert_eq!(
        stop_response.workload_status.workload_state,
        WorkloadState::Running,
        "workload should still be running after stopping the component"
    );

    // Verify component still exists
    assert_eq!(
        stop_response.workload_status.components.len(),
        1,
        "component should still exist in workload"
    );

    // Verify the stopped component has Stopped state
    let stopped_component = stop_response
        .workload_status
        .components
        .iter()
        .find(|c| c.component_id == component_to_stop)
        .expect("component should exist");
    assert_eq!(
        stopped_component.state,
        ComponentState::Stopped,
        "stopped component should be in Stopped state"
    );

    println!("✅ Component-specific stop test passed");
    println!(
        "   Stopped component {} and kept in memory",
        component_to_stop
    );

    Ok(())
}

/// Test 3: Component restart after stop
/// Verifies that a stopped component can be restarted using workload_start with component_ids
#[tokio::test]
async fn test_component_restart_after_stop() -> Result<()> {
    let (host, _port) = create_test_host().await?;

    let workload_id = uuid::Uuid::new_v4().to_string();
    let workload = create_test_workload("component-restart-test");

    // Start the workload
    let start_response = host
        .workload_start(WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload: workload.clone(),
            component_ids: None,
        })
        .await
        .context("failed to start workload")?;

    // Get the component ID
    let component_id = start_response.workload_status.components[0]
        .component_id
        .clone();

    // Stop the component
    let _stop_response = host
        .workload_stop(WorkloadStopRequest {
            workload_id: workload_id.clone(),
            component_ids: Some(vec![component_id.clone()]),
        })
        .await
        .context("failed to stop component")?;

    // Verify component is stopped
    let status_after_stop = host
        .workload_status(WorkloadStatusRequest {
            workload_id: workload_id.clone(),
        })
        .await
        .context("failed to get status after stop")?;

    let stopped_component = status_after_stop
        .workload_status
        .components
        .iter()
        .find(|c| c.component_id == component_id)
        .expect("component should exist");
    assert_eq!(
        stopped_component.state,
        ComponentState::Stopped,
        "component should be Stopped before restart"
    );

    // Restart the component using workload_start with component_ids
    let restart_response = host
        .workload_start(WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload: workload.clone(),
            component_ids: Some(vec![component_id.clone()]),
        })
        .await
        .context("failed to restart component")?;

    // Verify the component is now Running again
    let restarted_component = restart_response
        .workload_status
        .components
        .iter()
        .find(|c| c.component_id == component_id)
        .expect("restarted component should exist");
    assert_eq!(
        restarted_component.state,
        ComponentState::Running,
        "component should be Running after restart"
    );

    println!("✅ Component restart test passed");
    println!(
        "   Successfully restarted component {} after stop",
        component_id
    );

    Ok(())
}

/// Test 4: workload_update delegates to workload_start
/// Verifies that workload_update correctly updates specific components
#[tokio::test]
async fn test_workload_update_specific_components() -> Result<()> {
    let (host, _port) = create_test_host().await?;

    let workload_id = uuid::Uuid::new_v4().to_string();
    let workload = create_test_workload("workload-update-test");

    // Start the workload
    let start_response = host
        .workload_start(WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload: workload.clone(),
            component_ids: None,
        })
        .await
        .context("failed to start workload")?;

    // Get the component ID to update
    let component_to_update = start_response.workload_status.components[0]
        .component_id
        .clone();

    // Use workload_update to update the component
    // Component names in the workload spec are automatically matched to running components
    let update_response = host
        .workload_update(WorkloadUpdateRequest {
            workload_id: workload_id.clone(),
            workload: workload.clone(),
        })
        .await
        .context("failed to update workload")?;

    // Verify the update succeeded and component is still running
    assert_eq!(
        update_response.workload_status.workload_state,
        WorkloadState::Running,
        "workload should be running after update"
    );

    // Verify the updated component is in Running state
    let updated_component = update_response
        .workload_status
        .components
        .iter()
        .find(|c| c.component_id == component_to_update)
        .expect("updated component should exist");
    assert_eq!(
        updated_component.state,
        ComponentState::Running,
        "updated component should be Running"
    );

    println!("✅ workload_update test passed");
    println!(
        "   Successfully updated component {} via workload_update",
        component_to_update
    );

    Ok(())
}

/// Test 5: Full lifecycle - Start → Stop → Restart
/// Tests the complete component lifecycle with state transitions
#[tokio::test]
async fn test_full_component_lifecycle() -> Result<()> {
    let (host, port) = create_test_host().await?;

    let workload_id = uuid::Uuid::new_v4().to_string();
    let workload = create_test_workload("full-lifecycle-test");

    println!("🔄 Testing full component lifecycle");

    // Step 1: Start workload
    let start_response = host
        .workload_start(WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload: workload.clone(),
            component_ids: None,
        })
        .await
        .context("failed to start workload")?;

    let component_id = start_response.workload_status.components[0]
        .component_id
        .clone();

    // Verify component is Running after start
    assert_eq!(
        start_response.workload_status.components[0].state,
        ComponentState::Running,
        "component should be Running after start"
    );
    println!("   ✓ Step 1: Component started (Running)");

    // Test that HTTP endpoint works while component is running
    let client = reqwest::Client::new();
    let first_response = timeout(
        Duration::from_secs(5),
        client.get(format!("http://127.0.0.1:{port}/")).send(),
    )
    .await
    .context("HTTP request timed out")?
    .context("HTTP request failed")?;
    assert!(
        first_response.status().is_success(),
        "HTTP request should succeed while component is running"
    );
    println!("   ✓ HTTP endpoint works while running");

    // Step 2: Stop the specific component
    let stop_response = host
        .workload_stop(WorkloadStopRequest {
            workload_id: workload_id.clone(),
            component_ids: Some(vec![component_id.clone()]),
        })
        .await
        .context("failed to stop component")?;

    // Verify component is Stopped
    let stopped_component = stop_response
        .workload_status
        .components
        .iter()
        .find(|c| c.component_id == component_id)
        .expect("component should exist after stop");
    assert_eq!(
        stopped_component.state,
        ComponentState::Stopped,
        "component should be Stopped after stop"
    );
    println!("   ✓ Step 2: Component stopped (Stopped)");

    // Step 3: Restart the component
    let restart_response = host
        .workload_start(WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload: workload.clone(),
            component_ids: Some(vec![component_id.clone()]),
        })
        .await
        .context("failed to restart component")?;

    // Verify component is Running again
    let restarted_component = restart_response
        .workload_status
        .components
        .iter()
        .find(|c| c.component_id == component_id)
        .expect("component should exist after restart");
    assert_eq!(
        restarted_component.state,
        ComponentState::Running,
        "component should be Running after restart"
    );
    println!("   ✓ Step 3: Component restarted (Running)");

    // Verify HTTP endpoint works again after restart
    let second_response = timeout(
        Duration::from_secs(5),
        client.get(format!("http://127.0.0.1:{port}/")).send(),
    )
    .await
    .context("HTTP request timed out after restart")?
    .context("HTTP request failed after restart")?;
    assert!(
        second_response.status().is_success(),
        "HTTP request should succeed after component restart"
    );
    println!("   ✓ HTTP endpoint works after restart");

    println!("✅ Full component lifecycle test passed");

    Ok(())
}

/// Test 6: Error handling - stop non-existent component
/// Verifies graceful handling when trying to stop a component that doesn't exist
#[tokio::test]
async fn test_stop_nonexistent_component() -> Result<()> {
    let (host, _port) = create_test_host().await?;

    let workload_id = uuid::Uuid::new_v4().to_string();
    let workload = create_test_workload("error-handling-test");

    // Start the workload
    let _start_response = host
        .workload_start(WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload,
            component_ids: None,
        })
        .await
        .context("failed to start workload")?;

    // Try to stop a non-existent component - should not fail but should be a no-op
    let stop_response = host
        .workload_stop(WorkloadStopRequest {
            workload_id: workload_id.clone(),
            component_ids: Some(vec!["non-existent-component-id".to_string()]),
        })
        .await
        .context("stop request should not fail for non-existent component")?;

    // Workload should still be running
    assert_eq!(
        stop_response.workload_status.workload_state,
        WorkloadState::Running,
        "workload should still be running"
    );

    println!("✅ Error handling test passed");
    println!("   Gracefully handled attempt to stop non-existent component");

    Ok(())
}

/// Test 7: Component-specific start on non-running workload should fail
/// Verifies that trying to restart specific components on a non-existent workload fails appropriately
#[tokio::test]
async fn test_component_start_on_missing_workload() -> Result<()> {
    let (host, _port) = create_test_host().await?;

    let workload_id = uuid::Uuid::new_v4().to_string();
    let workload = create_test_workload("missing-workload-test");

    // Try to restart specific components on a workload that doesn't exist
    let result = host
        .workload_start(WorkloadStartRequest {
            workload_id,
            workload,
            component_ids: Some(vec!["some-component-id".to_string()]),
        })
        .await;

    // Should fail because workload doesn't exist
    assert!(
        result.is_err(),
        "should fail when trying to restart components on non-existent workload"
    );

    println!("✅ Missing workload error handling test passed");

    Ok(())
}

/// Test 8: Multi-component workload with selective component restart
/// Verifies that we can stop and restart individual components in a multi-component workload
#[tokio::test]
async fn test_multi_component_selective_restart() -> Result<()> {
    let (host, _port) = create_test_host().await?;

    let workload_id = uuid::Uuid::new_v4().to_string();
    let workload = create_multi_component_workload("multi-component-test");

    println!("🔄 Testing multi-component workload with selective restart");

    // Start the workload with 2 components
    let start_response = host
        .workload_start(WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload: workload.clone(),
            component_ids: None,
        })
        .await
        .context("failed to start multi-component workload")?;

    // Verify we have 2 components
    assert_eq!(
        start_response.workload_status.components.len(),
        2,
        "workload should have 2 components"
    );

    // Both should be Running
    for component in &start_response.workload_status.components {
        assert_eq!(
            component.state,
            ComponentState::Running,
            "all components should be Running after start"
        );
    }
    println!("   ✓ Step 1: Started 2 components (both Running)");

    // Get component IDs
    let component1_id = start_response.workload_status.components[0]
        .component_id
        .clone();
    let component2_id = start_response.workload_status.components[1]
        .component_id
        .clone();

    // Stop only the first component
    let stop_response = host
        .workload_stop(WorkloadStopRequest {
            workload_id: workload_id.clone(),
            component_ids: Some(vec![component1_id.clone()]),
        })
        .await
        .context("failed to stop first component")?;

    // Verify first component is Stopped, second is still Running
    let stopped_comp = stop_response
        .workload_status
        .components
        .iter()
        .find(|c| c.component_id == component1_id)
        .expect("component1 should exist");
    assert_eq!(
        stopped_comp.state,
        ComponentState::Stopped,
        "component1 should be Stopped"
    );

    let running_comp = stop_response
        .workload_status
        .components
        .iter()
        .find(|c| c.component_id == component2_id)
        .expect("component2 should exist");
    assert_eq!(
        running_comp.state,
        ComponentState::Running,
        "component2 should still be Running"
    );
    println!("   ✓ Step 2: Stopped component1, component2 still Running");

    // Restart only the first component
    let restart_response = host
        .workload_start(WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload: workload.clone(),
            component_ids: Some(vec![component1_id.clone()]),
        })
        .await
        .context("failed to restart first component")?;

    // Verify first component is now Running again
    let restarted_comp = restart_response
        .workload_status
        .components
        .iter()
        .find(|c| c.component_id == component1_id)
        .expect("component1 should exist");
    assert_eq!(
        restarted_comp.state,
        ComponentState::Running,
        "component1 should be Running after restart"
    );

    // Verify second component is still Running
    let still_running = restart_response
        .workload_status
        .components
        .iter()
        .find(|c| c.component_id == component2_id)
        .expect("component2 should exist");
    assert_eq!(
        still_running.state,
        ComponentState::Running,
        "component2 should still be Running"
    );
    println!("   ✓ Step 3: Restarted component1, both components Running");

    // Now stop the second component
    let stop2_response = host
        .workload_stop(WorkloadStopRequest {
            workload_id: workload_id.clone(),
            component_ids: Some(vec![component2_id.clone()]),
        })
        .await
        .context("failed to stop second component")?;

    let comp1_state = stop2_response
        .workload_status
        .components
        .iter()
        .find(|c| c.component_id == component1_id)
        .expect("component1 should exist");
    let comp2_state = stop2_response
        .workload_status
        .components
        .iter()
        .find(|c| c.component_id == component2_id)
        .expect("component2 should exist");

    assert_eq!(
        comp1_state.state,
        ComponentState::Running,
        "component1 should still be Running"
    );
    assert_eq!(
        comp2_state.state,
        ComponentState::Stopped,
        "component2 should be Stopped"
    );
    println!("   ✓ Step 4: Stopped component2, component1 still Running");

    // Restart the first component (HTTP counter) which uses the plugin interface
    let restart_comp1_response = host
        .workload_start(WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload: workload.clone(),
            component_ids: Some(vec![component1_id.clone()]),
        })
        .await
        .context("failed to restart component1")?;

    // Verify component1 is Running
    let comp1_restarted = restart_comp1_response
        .workload_status
        .components
        .iter()
        .find(|c| c.component_id == component1_id)
        .expect("component1 should exist");
    assert_eq!(
        comp1_restarted.state,
        ComponentState::Running,
        "component1 should be Running after restart"
    );
    println!("   ✓ Step 5: Restarted component1 successfully");

    println!("✅ Multi-component selective restart test passed");

    Ok(())
}

/// Test 9: Component update by name
/// Verifies that we can update a component by referring to it by name instead of ID
#[tokio::test]
async fn test_component_update_by_name() -> Result<()> {
    let (host, _port) = create_test_host().await?;

    let workload_id = uuid::Uuid::new_v4().to_string();
    let workload = create_test_workload("component-name-update-test");

    println!("🔄 Testing component update by name");

    // Start the workload
    let start_response = host
        .workload_start(WorkloadStartRequest {
            workload_id: workload_id.clone(),
            workload: workload.clone(),
            component_ids: None,
        })
        .await
        .context("failed to start workload")?;

    // Verify component has the name "main-component"
    assert_eq!(start_response.workload_status.components.len(), 1);
    let component_info = &start_response.workload_status.components[0];
    assert_eq!(
        component_info.name.as_deref(),
        Some("main-component"),
        "component should have name 'main-component'"
    );
    let component_id = component_info.component_id.clone();
    println!("   ✓ Step 1: Component started with name 'main-component'");

    // Update the component - names in spec are automatically matched to running components
    let update_response = host
        .workload_update(WorkloadUpdateRequest {
            workload_id: workload_id.clone(),
            workload: workload.clone(),
        })
        .await
        .context("failed to update component by name")?;

    // Verify the component was updated and still has the same name
    let updated_component = update_response
        .workload_status
        .components
        .iter()
        .find(|c| c.component_id == component_id)
        .expect("component should exist after update");

    assert_eq!(
        updated_component.name.as_deref(),
        Some("main-component"),
        "component should still have name 'main-component' after update"
    );
    assert_eq!(
        updated_component.state,
        ComponentState::Running,
        "component should be Running after update"
    );
    println!("   ✓ Step 2: Component updated by name successfully");

    println!("✅ Component update by name test passed");

    Ok(())
}
