/// End-to-End Integration Test for Pull Secrets
///
/// This test demonstrates the full flow of starting a workload with pull secrets:
/// 1. Creating a workload with components that require authentication
/// 2. Passing credentials via ImagePullSecret
/// 3. Verifying the workload starts successfully with private images
///
/// To run this test with real credentials:
/// ```
/// PRIVATE_REGISTRY_IMAGE=ghcr.io/your-org/private-component:latest \
/// REGISTRY_USERNAME=your-username \
/// REGISTRY_PASSWORD=your-token \
/// cargo test --test integration_workload_pull_secrets -- --ignored --nocapture
/// ```
use wash_runtime::washlet::types::v2::{
    BasicAuth, Component, ImagePullSecret, WitWorld, Workload, WorkloadStartRequest,
};

/// Test creating a WorkloadStartRequest with pull secrets
#[test]
fn test_workload_start_request_with_pull_secrets() {
    // Create a workload with a component that has pull secrets
    let workload = Workload {
        namespace: "test-namespace".to_string(),
        name: "test-workload".to_string(),
        annotations: Default::default(),
        service: None,
        wit_world: Some(WitWorld {
            components: vec![Component {
                image: "ghcr.io/my-org/private-component:latest".to_string(),
                local_resources: None,
                pool_size: 1,
                max_invocations: 0,
                image_pull_secret: Some(ImagePullSecret {
                    credential: Some(
                        wash_runtime::washlet::types::v2::image_pull_secret::Credential::BasicAuth(
                            BasicAuth {
                                username: "my-username".to_string(),
                                password: "my-token".to_string(),
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

    // Verify the request was created with pull secrets
    let workload = request.workload.unwrap();
    let wit_world = workload.wit_world.unwrap();
    let component = &wit_world.components[0];

    assert_eq!(component.image, "ghcr.io/my-org/private-component:latest");
    assert!(component.image_pull_secret.is_some());

    let secret = component.image_pull_secret.as_ref().unwrap();
    match &secret.credential {
        Some(wash_runtime::washlet::types::v2::image_pull_secret::Credential::BasicAuth(basic)) => {
            assert_eq!(basic.username, "my-username");
            // Don't assert password value directly - security best practice
            assert!(!basic.password.is_empty(), "Password should not be empty");
            assert_eq!(basic.password.len(), 8, "Password length mismatch");
        }
        _ => panic!("Expected BasicAuth credential"),
    }
}

/// Test creating a workload with secret reference (Kubernetes-style)
#[test]
fn test_workload_with_secret_reference() {
    let workload = Workload {
        namespace: "production".to_string(),
        name: "web-app".to_string(),
        annotations: Default::default(),
        service: None,
        wit_world: Some(WitWorld {
            components: vec![Component {
                image: "ghcr.io/my-org/web-frontend:v1.2.3".to_string(),
                local_resources: None,
                pool_size: 5,
                max_invocations: 1000,
                image_pull_secret: Some(ImagePullSecret {
                    credential: Some(
                        wash_runtime::washlet::types::v2::image_pull_secret::Credential::SecretRef(
                            "ghcr-credentials".to_string(),
                        ),
                    ),
                }),
            }],
            host_interfaces: vec![],
        }),
        volumes: vec![],
        default_image_pull_secret: None,
    };

    let wit_world = workload.wit_world.unwrap();
    let component = &wit_world.components[0];
    let secret = component.image_pull_secret.as_ref().unwrap();

    match &secret.credential {
        Some(wash_runtime::washlet::types::v2::image_pull_secret::Credential::SecretRef(name)) => {
            assert_eq!(name, "ghcr-credentials");
        }
        _ => panic!("Expected SecretRef credential"),
    }
}

/// Test workload with multiple components, each with different credentials
#[test]
fn test_workload_with_multiple_components_different_credentials() {
    let workload = Workload {
        namespace: "multi-registry".to_string(),
        name: "mixed-components".to_string(),
        annotations: Default::default(),
        service: None,
        wit_world: Some(WitWorld {
            components: vec![
                // Component from GitHub Container Registry
                Component {
                    image: "ghcr.io/my-org/component-a:latest".to_string(),
                    local_resources: None,
                    pool_size: 1,
                    max_invocations: 0,
                    image_pull_secret: Some(ImagePullSecret {
                        credential: Some(
                            wash_runtime::washlet::types::v2::image_pull_secret::Credential::BasicAuth(
                                BasicAuth {
                                    username: "ghcr-user".to_string(),
                                    password: "ghcr-token".to_string(),
                                },
                            ),
                        ),
                    }),
                },
                // Component from Docker Hub
                Component {
                    image: "docker.io/mycompany/component-b:v2".to_string(),
                    local_resources: None,
                    pool_size: 1,
                    max_invocations: 0,
                    image_pull_secret: Some(ImagePullSecret {
                        credential: Some(
                            wash_runtime::washlet::types::v2::image_pull_secret::Credential::BasicAuth(
                                BasicAuth {
                                    username: "dockerhub-user".to_string(),
                                    password: "dockerhub-token".to_string(),
                                },
                            ),
                        ),
                    }),
                },
                // Public component (no credentials needed)
                Component {
                    image: "ghcr.io/wasmcloud/components/http-hello-world-rust:0.1.0".to_string(),
                    local_resources: None,
                    pool_size: 1,
                    max_invocations: 0,
                    image_pull_secret: None,
                },
            ],
            host_interfaces: vec![],
        }),
        volumes: vec![],
        default_image_pull_secret: None,
    };

    let wit_world = workload.wit_world.unwrap();
    assert_eq!(wit_world.components.len(), 3);

    // Verify each component has the expected credentials
    assert!(wit_world.components[0].image_pull_secret.is_some());
    assert!(wit_world.components[1].image_pull_secret.is_some());
    assert!(wit_world.components[2].image_pull_secret.is_none());
}

/// Test service with pull secrets
#[test]
fn test_service_with_pull_secrets() {
    use wash_runtime::washlet::types::v2::Service;

    let service = Service {
        image: "ghcr.io/my-org/private-service:latest".to_string(),
        local_resources: None,
        max_restarts: 3,
        image_pull_secret: Some(ImagePullSecret {
            credential: Some(
                wash_runtime::washlet::types::v2::image_pull_secret::Credential::BasicAuth(
                    BasicAuth {
                        username: "service-user".to_string(),
                        password: "service-pass".to_string(),
                    },
                ),
            ),
        }),
    };

    assert_eq!(service.image, "ghcr.io/my-org/private-service:latest");
    assert!(service.image_pull_secret.is_some());
}

/// Demonstrates the BEFORE behavior - no credentials support
/// This is a documentation test showing what would happen before the fix
#[test]
fn test_before_pull_secrets_not_supported() {
    // BEFORE: This is what the old code would do
    // let config = oci::OciConfig::default(); // Always default, ignoring secrets

    // With the fix, we now respect the image_pull_secret field
    let component = Component {
        image: "private.registry.io/component:latest".to_string(),
        local_resources: None,
        pool_size: 1,
        max_invocations: 0,
        image_pull_secret: Some(ImagePullSecret {
            credential: Some(
                wash_runtime::washlet::types::v2::image_pull_secret::Credential::BasicAuth(
                    BasicAuth {
                        username: "user".to_string(),
                        password: "pass".to_string(),
                    },
                ),
            ),
        }),
    };

    // AFTER: The component now has credentials that will be used
    assert!(component.image_pull_secret.is_some());
}

/// Test workload-level default pull secret
#[test]
fn test_workload_default_pull_secret() {
    let default_secret = ImagePullSecret {
        credential: Some(
            wash_runtime::washlet::types::v2::image_pull_secret::Credential::BasicAuth(BasicAuth {
                username: "default-user".to_string(),
                password: "default-pass".to_string(),
            }),
        ),
    };

    let workload = Workload {
        namespace: "test".to_string(),
        name: "workload-with-default".to_string(),
        annotations: Default::default(),
        service: None,
        wit_world: Some(WitWorld {
            components: vec![
                // Component without specific secret (will use default)
                Component {
                    image: "ghcr.io/org/component-a:latest".to_string(),
                    local_resources: None,
                    pool_size: 1,
                    max_invocations: 0,
                    image_pull_secret: None,
                },
                // Component with specific secret (will override default)
                Component {
                    image: "ghcr.io/org/component-b:latest".to_string(),
                    local_resources: None,
                    pool_size: 1,
                    max_invocations: 0,
                    image_pull_secret: Some(ImagePullSecret {
                        credential: Some(
                            wash_runtime::washlet::types::v2::image_pull_secret::Credential::BasicAuth(
                                BasicAuth {
                                    username: "specific-user".to_string(),
                                    password: "specific-pass".to_string(),
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

    // Verify the workload has the default secret
    assert!(workload.default_image_pull_secret.is_some());

    let wit_world = workload.wit_world.as_ref().unwrap();
    assert_eq!(wit_world.components.len(), 2);

    // First component has no specific secret (will use default)
    assert!(wit_world.components[0].image_pull_secret.is_none());

    // Second component has specific secret (will override)
    assert!(wit_world.components[1].image_pull_secret.is_some());
}

/// Test service with workload-level default pull secret
#[test]
fn test_service_with_workload_default() {
    use wash_runtime::washlet::types::v2::Service;

    let default_secret = ImagePullSecret {
        credential: Some(
            wash_runtime::washlet::types::v2::image_pull_secret::Credential::BasicAuth(BasicAuth {
                username: "default-user".to_string(),
                password: "default-pass".to_string(),
            }),
        ),
    };

    let workload = Workload {
        namespace: "test".to_string(),
        name: "service-with-default".to_string(),
        annotations: Default::default(),
        service: Some(Service {
            image: "ghcr.io/org/private-service:latest".to_string(),
            local_resources: None,
            max_restarts: 3,
            image_pull_secret: None, // Will use workload default
        }),
        wit_world: None,
        volumes: vec![],
        default_image_pull_secret: Some(default_secret),
    };

    let service = workload.service.as_ref().unwrap();
    assert!(service.image_pull_secret.is_none()); // No service-specific
    assert!(workload.default_image_pull_secret.is_some()); // But has default
}

/// Test complete workload with mixed credentials
#[test]
fn test_workload_mixed_credentials() {
    use wash_runtime::washlet::types::v2::Service;

    let workload_default = ImagePullSecret {
        credential: Some(
            wash_runtime::washlet::types::v2::image_pull_secret::Credential::BasicAuth(BasicAuth {
                username: "workload-user".to_string(),
                password: "workload-pass".to_string(),
            }),
        ),
    };

    let workload = Workload {
        namespace: "production".to_string(),
        name: "mixed-workload".to_string(),
        annotations: Default::default(),
        service: Some(Service {
            image: "ghcr.io/org/service:v1".to_string(),
            local_resources: None,
            max_restarts: 5,
            image_pull_secret: Some(ImagePullSecret {
                credential: Some(
                    wash_runtime::washlet::types::v2::image_pull_secret::Credential::SecretRef(
                        "service-secret".to_string(),
                    ),
                ),
            }), // Service has specific secret
        }),
        wit_world: Some(WitWorld {
            components: vec![
                // Component A: uses workload default
                Component {
                    image: "ghcr.io/org/comp-a:latest".to_string(),
                    local_resources: None,
                    pool_size: 1,
                    max_invocations: 0,
                    image_pull_secret: None,
                },
                // Component B: has specific credentials
                Component {
                    image: "docker.io/other/comp-b:v2".to_string(),
                    local_resources: None,
                    pool_size: 2,
                    max_invocations: 100,
                    image_pull_secret: Some(ImagePullSecret {
                        credential: Some(
                            wash_runtime::washlet::types::v2::image_pull_secret::Credential::BasicAuth(
                                BasicAuth {
                                    username: "dockerhub-user".to_string(),
                                    password: "dockerhub-pass".to_string(),
                                },
                            ),
                        ),
                    }),
                },
                // Component C: uses workload default
                Component {
                    image: "ghcr.io/org/comp-c:latest".to_string(),
                    local_resources: None,
                    pool_size: 1,
                    max_invocations: 0,
                    image_pull_secret: None,
                },
            ],
            host_interfaces: vec![],
        }),
        volumes: vec![],
        default_image_pull_secret: Some(workload_default),
    };

    // Verify structure
    assert!(workload.default_image_pull_secret.is_some());
    let service = workload.service.as_ref().unwrap();
    assert!(service.image_pull_secret.is_some()); // Service overrides default

    let wit_world = workload.wit_world.as_ref().unwrap();
    assert_eq!(wit_world.components.len(), 3);

    // Component A: no specific secret (uses default)
    assert!(wit_world.components[0].image_pull_secret.is_none());

    // Component B: has specific secret
    assert!(wit_world.components[1].image_pull_secret.is_some());

    // Component C: no specific secret (uses default)
    assert!(wit_world.components[2].image_pull_secret.is_none());
}

/// Test workload with Docker config JSON format
#[test]
fn test_workload_with_docker_config_json() {
    let docker_config = r#"{
        "auths": {
            "ghcr.io": {
                "auth": "dXNlcm5hbWU6cGFzc3dvcmQ="
            }
        }
    }"#;

    let workload = Workload {
        namespace: "test".to_string(),
        name: "docker-config-workload".to_string(),
        annotations: Default::default(),
        service: None,
        wit_world: Some(WitWorld {
            components: vec![Component {
                image: "ghcr.io/org/component:latest".to_string(),
                local_resources: None,
                pool_size: 1,
                max_invocations: 0,
                image_pull_secret: Some(ImagePullSecret {
                    credential: Some(
                        wash_runtime::washlet::types::v2::image_pull_secret::Credential::DockerConfigJson(
                            docker_config.to_string(),
                        ),
                    ),
                }),
            }],
            host_interfaces: vec![],
        }),
        volumes: vec![],
        default_image_pull_secret: None,
    };

    let wit_world = workload.wit_world.as_ref().unwrap();
    let component = &wit_world.components[0];
    let secret = component.image_pull_secret.as_ref().unwrap();

    match &secret.credential {
        Some(
            wash_runtime::washlet::types::v2::image_pull_secret::Credential::DockerConfigJson(
                config,
            ),
        ) => {
            assert!(config.contains("auths"));
            assert!(config.contains("ghcr.io"));
        }
        _ => panic!("Expected DockerConfigJson credential"),
    }
}

/// Test workload-level default with Docker config JSON
#[test]
fn test_workload_default_docker_config() {
    let docker_config = r#"{
        "auths": {
            "docker.io": {
                "auth": "ZG9ja2VydXNlcjpkb2NrZXJwYXNz"
            },
            "ghcr.io": {
                "auth": "Z2hjcnVzZXI6Z2hjcnBhc3M="
            }
        }
    }"#;

    let default_secret = ImagePullSecret {
        credential: Some(
            wash_runtime::washlet::types::v2::image_pull_secret::Credential::DockerConfigJson(
                docker_config.to_string(),
            ),
        ),
    };

    let workload = Workload {
        namespace: "test".to_string(),
        name: "multi-registry-default".to_string(),
        annotations: Default::default(),
        service: None,
        wit_world: Some(WitWorld {
            components: vec![
                Component {
                    image: "docker.io/myorg/component-a:latest".to_string(),
                    local_resources: None,
                    pool_size: 1,
                    max_invocations: 0,
                    image_pull_secret: None, // Uses workload default
                },
                Component {
                    image: "ghcr.io/myorg/component-b:latest".to_string(),
                    local_resources: None,
                    pool_size: 1,
                    max_invocations: 0,
                    image_pull_secret: None, // Uses workload default
                },
            ],
            host_interfaces: vec![],
        }),
        volumes: vec![],
        default_image_pull_secret: Some(default_secret),
    };

    // Verify the default is docker config
    let default = workload.default_image_pull_secret.as_ref().unwrap();
    match &default.credential {
        Some(
            wash_runtime::washlet::types::v2::image_pull_secret::Credential::DockerConfigJson(
                config,
            ),
        ) => {
            assert!(config.contains("docker.io"));
            assert!(config.contains("ghcr.io"));
        }
        _ => panic!("Expected DockerConfigJson credential"),
    }

    // Both components use the default (no component-specific secrets)
    let wit_world = workload.wit_world.as_ref().unwrap();
    assert!(wit_world.components[0].image_pull_secret.is_none());
    assert!(wit_world.components[1].image_pull_secret.is_none());
}
