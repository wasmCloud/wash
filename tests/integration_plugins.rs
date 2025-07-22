//! Integration tests for wash plugin system
//!
//! This test validates the plugin test command functionality using the wadm plugin.
//! It runs the plugin test command with various combinations of command and hook flags.

use anyhow::{Context, Result};
use std::path::PathBuf;

use wash::{
    cli::{
        CliCommand, CliContext,
        plugin::{PluginCommand, TestCommand},
    },
    runtime::bindings::plugin::exports::wasmcloud::wash::plugin::HookType,
};

/// Test the plugin test command with various combinations of flags
#[tokio::test]
async fn test_plugin_test_wadm_comprehensive() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let ctx = CliContext::new()
        .await
        .context("Failed to create CLI context")?;

    let wadm_plugin_path = PathBuf::from("plugins/wadm");

    // Verify the wadm plugin directory exists
    if !wadm_plugin_path.exists() {
        anyhow::bail!(
            "wadm plugin directory not found at {}",
            wadm_plugin_path.display()
        );
    }

    eprintln!("🧪 Testing wadm plugin at: {}", wadm_plugin_path.display());

    // Test 1: Basic plugin test without any command or hook flags
    eprintln!("🔍 Test 1: Basic plugin test (no flags)");
    let test_cmd_basic = TestCommand {
        plugin: wadm_plugin_path.clone(),
        args: vec![],
        hooks: vec![],
    };
    let plugin_cmd_basic = PluginCommand::Test(test_cmd_basic);

    let result_basic = plugin_cmd_basic
        .handle(&ctx)
        .await
        .context("Failed to execute basic plugin test")?;

    assert!(
        result_basic.is_success(),
        "Basic plugin test should succeed"
    );
    eprintln!("✅ Basic plugin test passed");

    // Test 2: Plugin test with --command wadm
    eprintln!("🔍 Test 2: Plugin test with --command wadm");
    let test_cmd_with_command = TestCommand {
        plugin: wadm_plugin_path.clone(),
        args: vec!["wadm".to_string()],
        hooks: vec![],
    };
    let plugin_cmd_with_command = PluginCommand::Test(test_cmd_with_command);

    let result_with_command = plugin_cmd_with_command
        .handle(&ctx)
        .await
        .context("Failed to execute plugin test with command")?;

    assert!(
        result_with_command.is_success(),
        "Plugin test with command should succeed"
    );
    eprintln!("✅ Plugin test with command passed");

    // Test 3: Plugin test with --hook afterdev
    eprintln!("🔍 Test 3: Plugin test with --hook afterdev");
    let test_cmd_with_hook = TestCommand {
        plugin: wadm_plugin_path.clone(),
        args: vec![],
        hooks: vec![HookType::AfterDev],
    };
    let plugin_cmd_with_hook = PluginCommand::Test(test_cmd_with_hook);

    let result_with_hook = plugin_cmd_with_hook
        .handle(&ctx)
        .await
        .context("Failed to execute plugin test with hook")?;

    assert!(
        result_with_hook.is_success(),
        "Plugin test with hook should succeed"
    );
    eprintln!("✅ Plugin test with hook passed");

    // Test 4: Plugin test with both --command wadm and --hook afterdev
    eprintln!("🔍 Test 4: Plugin test with both --command wadm and --hook afterdev");
    let test_cmd_with_both = TestCommand {
        plugin: wadm_plugin_path.clone(),
        args: vec!["wadm".to_string()],
        hooks: vec![HookType::AfterDev],
    };
    let plugin_cmd_with_both = PluginCommand::Test(test_cmd_with_both);

    let result_with_both = plugin_cmd_with_both
        .handle(&ctx)
        .await
        .context("Failed to execute plugin test with both command and hook")?;

    assert!(
        result_with_both.is_success(),
        "Plugin test with both command and hook should succeed"
    );
    eprintln!("✅ Plugin test with both command and hook passed");

    // Verify that all tests produced meaningful output
    let outputs = [
        &result_basic,
        &result_with_command,
        &result_with_hook,
        &result_with_both,
    ];

    for (i, output) in outputs.iter().enumerate() {
        eprintln!("📋 Test {} output: {}", i + 1, output.text());

        // Verify that the output contains expected content
        if let Some(json_value) = output.json() {
            assert!(
                json_value.get("success").is_some(),
                "Output should contain success field"
            );
            assert!(
                json_value.get("metadata").is_some(),
                "Output should contain metadata field"
            );
            eprintln!(
                "📊 Test {} metadata: {}",
                i + 1,
                json_value.get("metadata").unwrap()
            );
        }
    }

    eprintln!("🎉 All wadm plugin tests passed successfully!");
    Ok(())
}
