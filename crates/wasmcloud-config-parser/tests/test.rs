use std::path::PathBuf;

use semver::Version;
use wasmcloud_config::{self, get_config, ActorConfig, LanguageConfig, RustConfig, TypeConfig};
#[test]
fn basic_test() {
    let result = get_config(Some(PathBuf::from("./tests")));

    assert!(result.is_ok());

    let config = result.unwrap();

    assert_eq!(
        config.language,
        LanguageConfig::Rust(RustConfig {
            cargo_path: Some("./cargo".into()),
            target_path: Some("./target".into())
        })
    );

    assert_eq!(
        config.project_type,
        TypeConfig::Actor(ActorConfig {
            claims: Some(vec!["wasmcloud:httpserver".to_string()]),
            registry: Some("localhost:8080".to_string()),
            push_insecure: false,
            key_directory: Some(PathBuf::from("./keys")),
            filename: Some("testactor.wasm".to_string()),
            wasm_type: Some("wasm32-unknown-unknown".to_string()),
        })
    );

    assert_eq!(config.name, "testactor".to_string());
    assert_eq!(config.version, Version::parse("0.1.0").unwrap());
}
