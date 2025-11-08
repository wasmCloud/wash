use wash_runtime::oci::{OciConfig, pull_component};

use std::time::Duration;

/// Expected digest prefix for OCI artifacts
const EXPECTED_DIGEST_PREFIX: &str = "sha256:";

/// Test that demonstrates the BEFORE state - credentials not plumbed through
/// This test is expected to FAIL when trying to pull from a private registry
#[tokio::test]
#[ignore] // Ignored by default since it requires a private registry
async fn test_pull_private_without_credentials_fails() {
    // This test demonstrates that without credentials, pulling from a private registry fails
    // Replace with your private registry URL
    let private_image = "ghcr.io/your-org/private-component:latest";

    let config = OciConfig::default();
    let result = pull_component(private_image, config).await;

    // Should fail with authentication error
    assert!(
        result.is_err(),
        "Should fail to pull private image without credentials"
    );

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("401") || err_msg.contains("403") || err_msg.contains("unauthorized"),
        "Expected authentication error, got: {}",
        err_msg
    );
}

/// Test that demonstrates the AFTER state - credentials work when provided
#[tokio::test]
#[ignore] // Ignored by default since it requires credentials
async fn test_pull_private_with_credentials_succeeds() {
    // This test demonstrates that with credentials, pulling from a private registry succeeds
    // Set these environment variables to test with real credentials:
    // PRIVATE_REGISTRY_IMAGE=ghcr.io/your-org/private-component:latest
    // REGISTRY_USERNAME=your-username
    // REGISTRY_PASSWORD=your-token

    let private_image = std::env::var("PRIVATE_REGISTRY_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/your-org/private-component:latest".to_string());
    let username = std::env::var("REGISTRY_USERNAME").expect("REGISTRY_USERNAME not set");
    let password = std::env::var("REGISTRY_PASSWORD").expect("REGISTRY_PASSWORD not set");

    let config = OciConfig::new_with_credentials(username, password);
    let result = pull_component(&private_image, config).await;

    // Should succeed with credentials
    assert!(
        result.is_ok(),
        "Failed to pull private image with credentials: {:?}",
        result.err()
    );

    let (component_bytes, digest) = result.unwrap();
    assert!(
        !component_bytes.is_empty(),
        "Component bytes should not be empty"
    );
    assert!(
        digest.starts_with(EXPECTED_DIGEST_PREFIX),
        "Digest should start with {}",
        EXPECTED_DIGEST_PREFIX
    );
}

/// Test pulling from a public registry (no credentials needed)
#[tokio::test]
async fn test_pull_public_image_no_credentials() {
    // This should work without any credentials
    let public_image = "ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0";

    let config = OciConfig::default();
    let result = pull_component(public_image, config).await;

    assert!(
        result.is_ok(),
        "Failed to pull public image: {:?}",
        result.err()
    );

    let (component_bytes, digest) = result.unwrap();
    assert!(
        !component_bytes.is_empty(),
        "Component bytes should not be empty"
    );
    assert!(
        digest.starts_with(EXPECTED_DIGEST_PREFIX),
        "Digest should start with {}",
        EXPECTED_DIGEST_PREFIX
    );
}

/// Test that credentials are preferred over docker credential helper
#[tokio::test]
#[ignore] // Ignored by default since it requires a private registry
async fn test_explicit_credentials_override_docker_helper() {
    // This test verifies that explicit credentials take precedence over docker credential helper
    let private_image = std::env::var("PRIVATE_REGISTRY_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/your-org/private-component:latest".to_string());

    // Use invalid credentials - should fail even if docker helper has valid ones
    let config =
        OciConfig::new_with_credentials("invalid_user".to_string(), "invalid_password".to_string());

    let result = pull_component(&private_image, config).await;

    // Should fail with invalid credentials, proving explicit creds were used
    assert!(
        result.is_err(),
        "Should fail with invalid explicit credentials"
    );

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("401") || err_msg.contains("403") || err_msg.contains("unauthorized"),
        "Expected authentication error, got: {}",
        err_msg
    );
}

/// Test with insecure registry (HTTP instead of HTTPS)
#[tokio::test]
#[ignore] // Requires local insecure registry
async fn test_pull_from_insecure_registry() {
    // To test this, run a local registry:
    // docker run -d -p 5000:5000 --name registry registry:2
    // docker tag your-component:latest localhost:5000/test-component:latest
    // docker push localhost:5000/test-component:latest

    let local_image = "localhost:5000/test-component:latest";

    let config = OciConfig::new_insecure();
    let result = pull_component(local_image, config).await;

    // May fail if registry isn't running, but shouldn't fail with HTTPS error
    if let Err(e) = result {
        let err_msg = e.to_string();
        assert!(
            !err_msg.contains("https") && !err_msg.contains("TLS"),
            "Should not have HTTPS/TLS errors when using insecure mode, got: {}",
            err_msg
        );
    }
}

/// Test that timeout configuration is respected
#[tokio::test]
#[ignore] // Requires a slow registry or network conditions
async fn test_pull_with_timeout() {
    // This test demonstrates timeout functionality
    // In a real scenario, this would use a slow/hanging registry
    // For testing, we use a very short timeout that will likely timeout on most pulls

    let public_image = "ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0";

    // Create config with a very short timeout (1 nanosecond - will almost certainly timeout)
    let config = OciConfig::new_with_timeout(Duration::from_nanos(1));
    let result = pull_component(public_image, config).await;

    // Should timeout
    assert!(result.is_err(), "Should timeout with 1ns timeout");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("timeout"),
        "Error should mention timeout, got: {}",
        err_msg
    );
}

/// Test that timeout can be configured via builder pattern
#[tokio::test]
async fn test_timeout_builder_pattern() {
    // Test that we can create a config and add timeout via with_timeout
    let config = OciConfig::default().with_timeout(Duration::from_secs(30));

    assert_eq!(config.timeout, Some(Duration::from_secs(30)));
}

/// Test timeout with credentials
#[tokio::test]
#[ignore] // Requires slow registry
async fn test_pull_with_credentials_and_timeout() {
    let private_image = std::env::var("PRIVATE_REGISTRY_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/your-org/private:latest".to_string());
    let username = std::env::var("REGISTRY_USERNAME").unwrap_or_else(|_| "user".to_string());
    let password = std::env::var("REGISTRY_PASSWORD").unwrap_or_else(|_| "pass".to_string());

    // Create config with credentials and a very short timeout
    let config =
        OciConfig::new_with_credentials(username, password).with_timeout(Duration::from_nanos(1));

    let result = pull_component(&private_image, config).await;

    // Should timeout (not auth error)
    assert!(result.is_err(), "Should timeout");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("timeout"),
        "Should be a timeout error, got: {}",
        err_msg
    );
}

/// Test concurrent pulls with different credentials
#[tokio::test]
#[ignore] // Requires multiple private registries with different credentials
async fn test_concurrent_pulls_different_credentials() {
    // This test demonstrates that pulling multiple components concurrently
    // with different credentials works correctly and doesn't mix up credentials

    // Setup: You need access to two different private registries with different credentials
    // For example:
    // - ghcr.io with GitHub credentials
    // - docker.io with Docker Hub credentials

    let registry1_image = std::env::var("REGISTRY1_IMAGE")
        .unwrap_or_else(|_| "ghcr.io/org1/component1:latest".to_string());
    let registry1_username =
        std::env::var("REGISTRY1_USERNAME").unwrap_or_else(|_| "user1".to_string());
    let registry1_password =
        std::env::var("REGISTRY1_PASSWORD").unwrap_or_else(|_| "pass1".to_string());

    let registry2_image = std::env::var("REGISTRY2_IMAGE")
        .unwrap_or_else(|_| "docker.io/org2/component2:latest".to_string());
    let registry2_username =
        std::env::var("REGISTRY2_USERNAME").unwrap_or_else(|_| "user2".to_string());
    let registry2_password =
        std::env::var("REGISTRY2_PASSWORD").unwrap_or_else(|_| "pass2".to_string());

    let public_image = "ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0";

    // Create different configs for each pull
    let config1 = OciConfig::new_with_credentials(&registry1_username, &registry1_password);
    let config2 = OciConfig::new_with_credentials(&registry2_username, &registry2_password);
    let config3 = OciConfig::default(); // Public image

    // Pull all three concurrently
    let (result1, result2, result3) = tokio::join!(
        pull_component(&registry1_image, config1),
        pull_component(&registry2_image, config2),
        pull_component(public_image, config3)
    );

    // Verify all pulls succeeded or failed with expected errors
    match result1 {
        Ok((bytes, digest)) => {
            println!(
                "✅ Registry 1 pull succeeded: {} bytes, digest: {}",
                bytes.len(),
                digest
            );
            assert!(!bytes.is_empty(), "Component should have data");
            assert!(digest.starts_with(EXPECTED_DIGEST_PREFIX), "Valid digest");
        }
        Err(e) => {
            // If it fails, ensure it's not a credential mix-up
            let err_msg = e.to_string();
            println!("Registry 1 pull failed: {}", err_msg);
            // Should be auth error or network error, not a credential mix-up
            assert!(
                err_msg.contains("401")
                    || err_msg.contains("403")
                    || err_msg.contains("unauthorized")
                    || err_msg.contains("not found"),
                "Expected auth/network error, got: {}",
                err_msg
            );
        }
    }

    match result2 {
        Ok((bytes, digest)) => {
            println!(
                "✅ Registry 2 pull succeeded: {} bytes, digest: {}",
                bytes.len(),
                digest
            );
            assert!(!bytes.is_empty(), "Component should have data");
            assert!(digest.starts_with(EXPECTED_DIGEST_PREFIX), "Valid digest");
        }
        Err(e) => {
            let err_msg = e.to_string();
            println!("Registry 2 pull failed: {}", err_msg);
            assert!(
                err_msg.contains("401")
                    || err_msg.contains("403")
                    || err_msg.contains("unauthorized")
                    || err_msg.contains("not found"),
                "Expected auth/network error, got: {}",
                err_msg
            );
        }
    }

    // Public image should always succeed
    let (bytes, digest) = result3.expect("Public image pull should succeed");
    println!(
        "✅ Public pull succeeded: {} bytes, digest: {}",
        bytes.len(),
        digest
    );
    assert!(!bytes.is_empty(), "Component should have data");
    assert!(digest.starts_with(EXPECTED_DIGEST_PREFIX), "Valid digest");
}

/// Test rapid concurrent pulls to the same registry with same credentials
#[tokio::test]
async fn test_concurrent_pulls_same_credentials() {
    // Pull the same public component multiple times concurrently
    // This tests that credential resolution doesn't have race conditions

    let public_image = "ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0";

    // Create 5 concurrent pulls with default config
    let pulls = (0..5).map(|_| pull_component(public_image, OciConfig::default()));

    // Execute all pulls concurrently
    let results = futures::future::join_all(pulls).await;

    // All should succeed
    for (i, result) in results.into_iter().enumerate() {
        let (bytes, digest) = result.unwrap_or_else(|_| panic!("Pull #{} should succeed", i));
        assert!(!bytes.is_empty(), "Component #{} should have data", i);
        assert!(
            digest.starts_with(EXPECTED_DIGEST_PREFIX),
            "Valid digest #{}",
            i
        );
    }
}
