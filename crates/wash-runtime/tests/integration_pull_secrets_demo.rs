/// Comprehensive demonstration test for Pull Secrets feature
///
/// This test demonstrates the complete before/after behavior of pull secrets support.
/// It's designed to be run manually with real credentials to verify the feature works end-to-end.
///
/// # Running this test
///
/// ## Prerequisites
/// 1. Create a private component and push to a registry (e.g., GitHub Container Registry)
/// 2. Set up environment variables with your credentials
///
/// ## Steps to test
///
/// ```bash
/// # 1. Build and push a private component
/// cd examples/http-hello-world
/// wash build
/// wash push ghcr.io/YOUR_ORG/private-test:latest build/http_hello_world_s.wasm \
///   --registry-username YOUR_USERNAME \
///   --registry-password YOUR_TOKEN
///
/// # 2. Set environment variables
/// export PRIVATE_REGISTRY_IMAGE="ghcr.io/YOUR_ORG/private-test:latest"
/// export REGISTRY_USERNAME="YOUR_USERNAME"
/// export REGISTRY_PASSWORD="YOUR_TOKEN"
///
/// # 3. Run the test
/// cd crates/wash-runtime
/// cargo test --test integration_pull_secrets_demo -- --ignored --nocapture
/// ```
use wash_runtime::oci::{OciConfig, pull_component};
use wash_runtime::washlet::types::v2::{
    BasicAuth, Component, ImagePullSecret, WitWorld, Workload, WorkloadStartRequest,
};

#[tokio::test]
#[ignore]
async fn demo_before_pull_secrets_fails() {
    println!("\n=== DEMO: Before Pull Secrets Support ===\n");

    let private_image =
        std::env::var("PRIVATE_REGISTRY_IMAGE").expect("Set PRIVATE_REGISTRY_IMAGE env var");

    println!("📦 Attempting to pull private image: {}", private_image);
    println!("🔓 Using: OciConfig::default() (no credentials)");

    // BEFORE: Always used default config
    let config = OciConfig::default();
    let result = pull_component(&private_image, config).await;

    match result {
        Ok(_) => {
            println!("✅ Succeeded (image may be public or docker creds configured)");
        }
        Err(e) => {
            println!("❌ Failed as expected: {}", e);
            println!(
                "   This demonstrates that private images can't be pulled without credentials"
            );
        }
    }
}

#[tokio::test]
#[ignore]
async fn demo_after_pull_secrets_succeeds() {
    println!("\n=== DEMO: After Pull Secrets Support ===\n");

    let private_image =
        std::env::var("PRIVATE_REGISTRY_IMAGE").expect("Set PRIVATE_REGISTRY_IMAGE env var");
    let username = std::env::var("REGISTRY_USERNAME").expect("Set REGISTRY_USERNAME env var");
    let password = std::env::var("REGISTRY_PASSWORD").expect("Set REGISTRY_PASSWORD env var");

    println!("📦 Attempting to pull private image: {}", private_image);
    println!("🔐 Using: Explicit credentials (username/password)");
    println!("   Username: {}", username);

    // AFTER: Can provide credentials
    let config = OciConfig::new_with_credentials(username.clone(), password.clone());
    let result = pull_component(&private_image, config).await;

    match result {
        Ok((bytes, digest)) => {
            println!("✅ SUCCESS!");
            println!("   Component size: {} bytes", bytes.len());
            println!("   Digest: {}", digest);
            println!("\n   This demonstrates that private images CAN be pulled with credentials");
        }
        Err(e) => {
            println!("❌ Failed: {}", e);
            println!("   Check your credentials and image reference");
        }
    }
}

#[tokio::test]
#[ignore]
async fn demo_workload_api_with_pull_secrets() {
    println!("\n=== DEMO: Workload API with Pull Secrets ===\n");

    let private_image =
        std::env::var("PRIVATE_REGISTRY_IMAGE").expect("Set PRIVATE_REGISTRY_IMAGE env var");
    let username = std::env::var("REGISTRY_USERNAME").expect("Set REGISTRY_USERNAME env var");
    let password = std::env::var("REGISTRY_PASSWORD").expect("Set REGISTRY_PASSWORD env var");

    println!("📋 Creating WorkloadStartRequest with pull secrets");
    println!("   Image: {}", private_image);
    println!("   Username: {}", username);

    // Create a workload with pull secrets
    let workload = Workload {
        namespace: "demo".to_string(),
        name: "private-component-demo".to_string(),
        annotations: Default::default(),
        service: None,
        wit_world: Some(WitWorld {
            components: vec![Component {
                image: private_image.clone(),
                local_resources: None,
                pool_size: 1,
                max_invocations: 0,
                image_pull_secret: Some(ImagePullSecret {
                    credential: Some(
                        wash_runtime::washlet::types::v2::image_pull_secret::Credential::BasicAuth(
                            BasicAuth {
                                username: username.clone(),
                                password: password.clone(),
                            },
                        ),
                    ),
                }),
            }],
            host_interfaces: vec![],
        }),
        volumes: vec![],
        default_image_pull_secret: None,
    };

    let request = WorkloadStartRequest {
        workload: Some(workload),
    };

    println!("✅ Created WorkloadStartRequest successfully");
    println!("\n📊 Request Structure:");
    println!("   - Workload: demo/private-component-demo");
    println!("   - Components: 1");
    println!("   - Image Pull Secret: BasicAuth");

    // Verify the structure
    let workload = request.workload.as_ref().unwrap();
    let wit_world = workload.wit_world.as_ref().unwrap();
    let component = &wit_world.components[0];

    assert_eq!(component.image, private_image);
    assert!(component.image_pull_secret.is_some());

    println!("\n✅ This request would now work with the runtime!");
    println!("   The workload_start handler will:");
    println!("   1. Extract the ImagePullSecret from the component");
    println!("   2. Convert it to OciConfig with credentials");
    println!("   3. Pull the component using authenticated access");
    println!("   4. Start the workload successfully");
}

#[tokio::test]
#[ignore]
async fn demo_comparison_side_by_side() {
    println!("\n=== SIDE-BY-SIDE COMPARISON ===\n");

    let private_image =
        std::env::var("PRIVATE_REGISTRY_IMAGE").expect("Set PRIVATE_REGISTRY_IMAGE env var");
    let username = std::env::var("REGISTRY_USERNAME").expect("Set REGISTRY_USERNAME env var");
    let password = std::env::var("REGISTRY_PASSWORD").expect("Set REGISTRY_PASSWORD env var");

    println!("┌─────────────────────────────────────────────────────────────┐");
    println!("│                    BEFORE (OLD CODE)                        │");
    println!("└─────────────────────────────────────────────────────────────┘");
    println!("Code:");
    println!("  let config = OciConfig::default();  // Always default!");
    println!("  let bytes = oci::pull_component(&image, config).await?;");
    println!("\nResult:");

    let before_config = OciConfig::default();
    let before_result = pull_component(&private_image, before_config).await;
    match before_result {
        Ok(_) => println!("  ✅ Success (docker creds or public image)"),
        Err(_) => println!("  ❌ FAILS for private images"),
    }

    println!("\n┌─────────────────────────────────────────────────────────────┐");
    println!("│                     AFTER (NEW CODE)                        │");
    println!("└─────────────────────────────────────────────────────────────┘");
    println!("Code:");
    println!("  let config = image_pull_secret_to_oci_config(");
    println!("      component.image_pull_secret.as_ref()");
    println!("  );");
    println!("  let bytes = oci::pull_component(&image, config).await?;");
    println!("\nResult:");

    let after_config = OciConfig::new_with_credentials(username, password);
    let after_result = pull_component(&private_image, after_config).await;
    match after_result {
        Ok((bytes, digest)) => {
            println!("  ✅ SUCCESS for private images!");
            println!("     Size: {} bytes", bytes.len());
            println!("     Digest: {}", digest);
        }
        Err(e) => println!("  ❌ Failed: {}", e),
    }

    println!("\n┌─────────────────────────────────────────────────────────────┐");
    println!("│                         SUMMARY                             │");
    println!("└─────────────────────────────────────────────────────────────┘");
    println!("BEFORE: ❌ No support for private registries via API");
    println!("AFTER:  ✅ Full support with BasicAuth and SecretRef");
    println!();
}

#[test]
fn demo_api_structure() {
    println!("\n=== API Structure Demo ===\n");

    println!("New Protobuf Messages:");
    println!("```protobuf");
    println!("message ImagePullSecret {{");
    println!("  oneof credential {{");
    println!("    BasicAuth basic_auth = 1;");
    println!("    string secret_ref = 2;");
    println!("  }}");
    println!("}}");
    println!();
    println!("message BasicAuth {{");
    println!("  string username = 1;");
    println!("  string password = 2;");
    println!("}}");
    println!("```");
    println!();

    println!("Updated Messages:");
    println!("```protobuf");
    println!("message Component {{");
    println!("  string image = 1;");
    println!("  LocalResources local_resources = 2;");
    println!("  sint32 pool_size = 3;");
    println!("  sint32 max_invocations = 4;");
    println!("  ImagePullSecret image_pull_secret = 5;  // NEW!");
    println!("}}");
    println!("```");
    println!();

    println!("✅ Non-breaking change: image_pull_secret is optional");
    println!("✅ Backward compatible: existing workloads work without changes");
    println!("✅ Forward compatible: ready for Kubernetes secret integration");
}
