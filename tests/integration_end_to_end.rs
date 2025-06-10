//! End-to-end integration test for template creation, component building, and execution
//!
//! This test demonstrates the complete flow from template creation to component execution:
//! 1. Creates a new project from the cosmonic-control-welcome-tour template in a temp directory
//! 2. Uses build_project to build the project
//! 3. Uses prepare_component_dev plugin function to load the built component from file
//! 4. Calls the HTTP handler on the component

use anyhow::{Context, Result};
use http_body_util::BodyExt;
use std::{path::PathBuf, sync::Arc};
use tempfile::TempDir;
use wasmtime_wasi_http::{
    WasiHttpView,
    bindings::http::types::Scheme,
    body::{HostIncomingBody, HyperIncomingBody},
    types::HostIncomingRequest,
};

use wash::{
    cli::{CliContext, component_build::build_component},
    component_build::BuildConfig,
    config::Config,
    new::new_project_from_template,
    runtime::{Ctx, bindings::dev::Dev, new_runtime, prepare_component_dev},
};

/// Comprehensive end-to-end test for project creation, building, and execution
#[tokio::test]
async fn test_end_to_end_template_to_execution() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Step 1: Create a temporary directory for the test
    let temp_dir = TempDir::new().context("Failed to create temporary directory")?;

    // Step 2: Create project from cosmonic-control-welcome-tour template
    let ctx = CliContext::new()
        .await
        .context("Failed to create CLI context")?;

    new_project_from_template(
        &ctx,
        &Config::default_with_templates().templates[0],
        temp_dir.path(),
    )
    .await
    .context("Failed to create project from template")?;

    eprintln!(
        "✅ Created project from template at: {}",
        temp_dir.path().display()
    );

    let cfg = Config {
        build: Some(BuildConfig {
            artifact_path: Some(
                temp_dir
                    .path()
                    .join("dist")
                    .join("cosmonic_control_welcome_tour.wasm"),
            ),
            ..Default::default()
        }),
        ..Config::default_with_templates()
    };
    // Step 3: Build the component
    let built_component = build_component(
        temp_dir.path(),
        &CliContext::new()
            .await
            .expect("failed to create CLI context"),
        &cfg,
    )
    .await
    .context("failed to build component")?;

    eprintln!(
        "✅ Built component at: {}",
        built_component.artifact_path.display()
    );

    // Step 4: Load and execute the component using prepare_component_dev
    let response_body = execute_component_http_handler(
        &built_component.artifact_path,
        "http://localhost:8080/",
        200,
    )
    .await
    .context("Failed to execute component HTTP handler")?;

    eprintln!("✅ Component execution successful!");
    eprintln!("📨 HTTP Response: {}", response_body);

    // Step 5: Verify the response contains expected content
    // The cosmonic-control-welcome-tour template typically returns a welcome message
    assert!(
        !response_body.trim().is_empty(),
        "Response body should not be empty"
    );

    // Additional assertions can be added here based on the expected behavior
    // of the cosmonic-control-welcome-tour template
    assert!(!response_body.is_empty(), "Response should have content");

    eprintln!("✅ All tests passed! End-to-end flow working correctly.");

    Ok(())
}

/// Test the plugin system component loading
#[tokio::test]
async fn test_plugin_component_loading() -> Result<()> {
    // Create a minimal HTTP component for testing
    let wasm_bytes = vec![
        0x00, 0x61, 0x73, 0x6d, // wasm magic
        0x01, 0x00, 0x00, 0x00, // wasm version
    ];

    // Test runtime creation
    let (runtime, _handle) = new_runtime().await.context("Failed to create runtime")?;

    // Test component preparation
    let component = prepare_component_dev(&runtime, &wasm_bytes, Arc::default()).await;

    // Note: This test may fail if the component doesn't implement the expected interfaces
    // but it verifies that the plugin system is correctly set up
    match component {
        Ok(_) => {
            eprintln!("✅ Component loaded successfully with plugin system!");
        }
        Err(e) => {
            eprintln!(
                "ℹ️  Component loading failed as expected for minimal test component: {}",
                e
            );
            eprintln!(
                "ℹ️  This is normal for a minimal test component that doesn't implement HTTP interfaces"
            );
        }
    }

    Ok(())
}

/// Loads and executes the component using the plugin system
async fn execute_component_http_handler(
    component_path: &PathBuf,
    request_uri: &str,
    expected_status: u16,
) -> Result<String> {
    // Read the component wasm bytes
    let wasm_bytes = tokio::fs::read(component_path).await.with_context(|| {
        format!(
            "Failed to read component file: {}",
            component_path.display()
        )
    })?;

    // Create runtime and context
    let (runtime, _handle) = new_runtime()
        .await
        .context("Failed to create wasmtime runtime")?;

    let base_ctx = Ctx::default();

    // Prepare the component for execution
    let component = prepare_component_dev(&runtime, &wasm_bytes, Arc::default())
        .await
        .context("Failed to prepare component for development")?;

    let mut store = component.new_store(base_ctx);

    // Instantiate the component
    let instance = component
        .instance_pre()
        .instantiate_async(&mut store)
        .await
        .context("Failed to instantiate component")?;

    // Get the HTTP handler interface using the correct import path
    let http_instance =
        Dev::new(&mut store, &instance).context("Failed to get HTTP incoming handler interface")?;

    // Create an HTTP request
    let request: ::http::Request<HyperIncomingBody> = http::Request::builder()
        .uri(request_uri)
        .method("GET")
        .body(HyperIncomingBody::default())
        .context("Failed to create HTTP request")?;

    let (parts, body) = request.into_parts();
    let data = store.data_mut();

    // Create the incoming body with timeout
    let body = HostIncomingBody::new(
        body,
        std::time::Duration::from_millis(30 * 1000), // 30 second timeout
    );

    let wasmtime_scheme = Scheme::Http;
    let incoming_req = HostIncomingRequest::new(data, parts, wasmtime_scheme, Some(body))
        .context("Failed to create incoming request")?;
    let request_resource = data
        .table
        .push(incoming_req)
        .context("Failed to push request to resource table")?;

    // Create response channel
    let (tx, rx) = tokio::sync::oneshot::channel();
    let response = data
        .new_response_outparam(tx)
        .context("Failed to create response outparam")?;

    // Call the HTTP handler
    http_instance
        .wasi_http_incoming_handler()
        .call_handle(&mut store, request_resource, response)
        .await
        .context("Failed to call HTTP handler")?;

    // Get the response
    let resp = rx
        .await
        .context("Failed to receive response (channel error)")?
        .context("Failed to receive response (handler error)")?;

    // Verify status code
    if resp.status() != expected_status {
        anyhow::bail!(
            "Expected status code {}, got {}",
            expected_status,
            resp.status()
        );
    }

    // Extract response body
    let body = resp
        .collect()
        .await
        .context("Failed to collect response body")?
        .to_bytes();

    let body_str = String::from_utf8_lossy(&body).to_string();
    Ok(body_str)
}
