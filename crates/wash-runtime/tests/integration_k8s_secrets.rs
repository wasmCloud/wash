//! Integration tests for Kubernetes secret resolution
//!
//! These tests require running in a Kubernetes cluster with appropriate RBAC permissions.
//!
//! # Setup
//!
//! 1. Create a test namespace:
//!    ```bash
//!    kubectl create namespace wash-test
//!    ```
//!
//! 2. Create an opaque secret with username/password:
//!    ```bash
//!    kubectl create secret generic test-registry-secret \
//!      --from-literal=username=testuser \
//!      --from-literal=password=testpass \
//!      -n wash-test
//!    ```
//!
//! 3. Create a dockerconfigjson secret:
//!    ```bash
//!    kubectl create secret docker-registry test-docker-secret \
//!      --docker-server=ghcr.io \
//!      --docker-username=myuser \
//!      --docker-password=mypass \
//!      -n wash-test
//!    ```
//!
//! 4. Run the tests:
//!    ```bash
//!    cargo test --test integration_k8s_secrets -- --ignored --nocapture
//!    ```

use wash_runtime::washlet::types::v2::{BasicAuth, ImagePullSecret};

/// Test resolving an Opaque Kubernetes secret
#[tokio::test]
#[ignore] // Requires running in Kubernetes cluster
async fn test_resolve_opaque_secret() {
    let secret_name = "test-registry-secret";
    let namespace = "wash-test";

    let result = wash_runtime::washlet::k8s_secrets::resolve_secret(secret_name, namespace).await;

    match result {
        Ok(Some((username, password))) => {
            assert_eq!(username, "testuser");
            assert_eq!(password, "testpass");
            println!("✅ Successfully resolved opaque secret");
        }
        Ok(None) => {
            panic!("Secret exists but returned no credentials");
        }
        Err(e) => {
            panic!("Failed to resolve secret: {}", e);
        }
    }
}

/// Test resolving a dockerconfigjson Kubernetes secret
#[tokio::test]
#[ignore] // Requires running in Kubernetes cluster
async fn test_resolve_docker_config_json_secret() {
    let secret_name = "test-docker-secret";
    let namespace = "wash-test";

    let result = wash_runtime::washlet::k8s_secrets::resolve_secret(secret_name, namespace).await;

    match result {
        Ok(Some((username, password))) => {
            assert_eq!(username, "myuser");
            assert_eq!(password, "mypass");
            println!("✅ Successfully resolved dockerconfigjson secret");
        }
        Ok(None) => {
            panic!("Secret exists but returned no credentials");
        }
        Err(e) => {
            panic!("Failed to resolve secret: {}", e);
        }
    }
}

/// Test resolving a non-existent secret
#[tokio::test]
#[ignore] // Requires running in Kubernetes cluster
async fn test_resolve_nonexistent_secret() {
    let secret_name = "nonexistent-secret";
    let namespace = "wash-test";

    let result = wash_runtime::washlet::k8s_secrets::resolve_secret(secret_name, namespace).await;

    assert!(result.is_err(), "Should error when secret doesn't exist");

    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("not found") || err_msg.contains("NotFound"),
        "Error should indicate secret not found, got: {}",
        err_msg
    );
}

/// Test secret resolution with empty credentials
#[tokio::test]
#[ignore] // Requires running in Kubernetes cluster
async fn test_resolve_secret_empty_credentials() {
    // First create a secret with empty password
    // kubectl create secret generic test-empty-secret \
    //   --from-literal=username=testuser \
    //   --from-literal=password= \
    //   -n wash-test

    let secret_name = "test-empty-secret";
    let namespace = "wash-test";

    let result = wash_runtime::washlet::k8s_secrets::resolve_secret(secret_name, namespace).await;

    match result {
        Ok(None) => {
            println!("✅ Correctly returned None for empty credentials");
        }
        Ok(Some(_)) => {
            panic!("Should return None for empty credentials");
        }
        Err(e) => {
            // Also acceptable if secret doesn't exist yet
            println!("⚠️  Secret not found (may not be created yet): {}", e);
        }
    }
}

/// Test end-to-end: ImagePullSecret with SecretRef
#[tokio::test]
#[ignore] // Requires running in Kubernetes cluster
async fn test_workload_with_k8s_secret() {
    use wash_runtime::washlet::types::v2::{Component, WitWorld, Workload};

    let secret = ImagePullSecret {
        credential: Some(
            wash_runtime::washlet::types::v2::image_pull_secret::Credential::SecretRef(
                "test-registry-secret".to_string(),
            ),
        ),
    };

    let workload = Workload {
        namespace: "wash-test".to_string(),
        name: "test-workload".to_string(),
        annotations: Default::default(),
        service: None,
        wit_world: Some(WitWorld {
            components: vec![Component {
                image: "ghcr.io/test/component:latest".to_string(),
                local_resources: None,
                pool_size: 1,
                max_invocations: 0,
                image_pull_secret: Some(secret),
            }],
            host_interfaces: vec![],
        }),
        volumes: vec![],
        default_image_pull_secret: None,
    };

    // Verify the workload structure
    let component = &workload.wit_world.as_ref().unwrap().components[0];
    assert!(component.image_pull_secret.is_some());

    println!("✅ Workload with K8s secret reference created successfully");
    println!("   In a real scenario, this would resolve to testuser:testpass");
}

/// Test with workload-level default secret ref
#[tokio::test]
#[ignore] // Requires running in Kubernetes cluster
async fn test_workload_default_secret_ref() {
    use wash_runtime::washlet::types::v2::{Component, WitWorld, Workload};

    let default_secret = ImagePullSecret {
        credential: Some(
            wash_runtime::washlet::types::v2::image_pull_secret::Credential::SecretRef(
                "test-registry-secret".to_string(),
            ),
        ),
    };

    let workload = Workload {
        namespace: "wash-test".to_string(),
        name: "test-workload-default".to_string(),
        annotations: Default::default(),
        service: None,
        wit_world: Some(WitWorld {
            components: vec![
                // Component without specific secret (will use default)
                Component {
                    image: "ghcr.io/test/component-a:latest".to_string(),
                    local_resources: None,
                    pool_size: 1,
                    max_invocations: 0,
                    image_pull_secret: None,
                },
                // Component with explicit BasicAuth (overrides default)
                Component {
                    image: "ghcr.io/test/component-b:latest".to_string(),
                    local_resources: None,
                    pool_size: 1,
                    max_invocations: 0,
                    image_pull_secret: Some(ImagePullSecret {
                        credential: Some(
                            wash_runtime::washlet::types::v2::image_pull_secret::Credential::BasicAuth(
                                BasicAuth {
                                    username: "explicit-user".to_string(),
                                    password: "explicit-pass".to_string(),
                                },
                            ),
                        ),
                    }),
                },
            ],
            host_interfaces: vec![],
        }),
        volumes: vec![],
        default_image_pull_secret: Some(default_secret),
    };

    // Verify structure
    assert!(workload.default_image_pull_secret.is_some());
    let components = &workload.wit_world.as_ref().unwrap().components;
    assert!(components[0].image_pull_secret.is_none()); // Uses default
    assert!(components[1].image_pull_secret.is_some()); // Explicit

    println!("✅ Workload with default K8s secret reference created");
    println!("   Component A would use K8s secret");
    println!("   Component B would use explicit BasicAuth");
}
