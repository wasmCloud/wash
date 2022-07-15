use std::path::PathBuf;

use claim::{assert_err, assert_ok};
use semver::Version;
use wasmcloud_config_parser::{
    self, get_config, ActorConfig, LanguageConfig, RustConfig, TinyGoConfig, TypeConfig,
};

#[test]
/// When given a specific toml file's path, it should parse the file and return a ProjectConfig.
fn rust_actor() {
    let result = get_config(Some(PathBuf::from("./tests/files/rust_actor.toml")), None);

    let config = assert_ok!(result);

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
            wasm_target: Some("wasm32-unknown-unknown".to_string()),
            call_alias: Some("testactor".to_string())
        })
    );

    assert_eq!(config.name, "testactor".to_string());
    assert_eq!(config.version, Version::parse("0.1.0").unwrap());
}

#[test]
fn tinygo_actor() {
    let result = get_config(Some(PathBuf::from("./tests/files/tinygo_actor.toml")), None);

    let config = assert_ok!(result);

    assert_eq!(
        config.language,
        LanguageConfig::TinyGo(TinyGoConfig {
            tinygo_path: Some("path/to/tinygo".into())
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
            wasm_target: Some("wasm32-unknown-unknown".to_string()),
            call_alias: Some("testactor".to_string())
        })
    );

    assert_eq!(config.name, "testactor".to_string());
    assert_eq!(config.version, Version::parse("0.1.0").unwrap());
}

#[test]
/// When given a folder, should automatically grab a wasmcloud.toml file inside it and parse it.
fn folder_path() {
    let result = get_config(Some(PathBuf::from("./tests/files/folder")), None);

    let config = assert_ok!(result);

    assert_eq!(
        config.language,
        LanguageConfig::Rust(RustConfig {
            cargo_path: Some("./cargo".into()),
            target_path: Some("./target".into())
        })
    );
}

#[test]
fn no_actor_config() {
    let result = get_config(Some(PathBuf::from("./tests/files/no_actor.toml")), None);

    let err = assert_err!(result);

    assert_eq!(
        "Missing actor config in ./tests/files/no_actor.toml",
        err.to_string().as_str()
    );
}

#[test]
fn no_provider_config() {
    let result = get_config(Some(PathBuf::from("./tests/files/no_provider.toml")), None);

    let err = assert_err!(result);

    assert_eq!(
        "Missing provider config in ./tests/files/no_provider.toml",
        err.to_string().as_str()
    );
}

#[test]
fn no_interface_config() {
    let result = get_config(Some(PathBuf::from("./tests/files/no_interface.toml")), None);

    let err = assert_err!(result);

    assert_eq!(
        "Missing interface config in ./tests/files/no_interface.toml",
        err.to_string().as_str()
    );
}

#[test]
/// When given a folder with no wasmcloud.toml file, should return an error.
fn folder_path_with_no_config() {
    let result = get_config(Some(PathBuf::from("./tests/files/noconfig")), None);

    let err = assert_err!(result);
    assert_eq!(
        "No wasmcloud.toml file found in ./tests/files/noconfig",
        err.to_string().as_str()
    );
}

#[test]
/// When given a random file, should return an error.
fn random_file() {
    let result = get_config(Some(PathBuf::from("./tests/files/random.txt")), None);

    let err = assert_err!(result);
    assert_eq!(
        "Invalid config file: ./tests/files/random.txt",
        err.to_string().as_str()
    );
}

#[test]
/// When given a nonexistent file or path, should return an error.
fn nonexistent_file() {
    let result = get_config(Some(PathBuf::from("./tests/files/nonexistent.toml")), None);

    let err = assert_err!(result);
    assert_eq!(
        "Path ./tests/files/nonexistent.toml does not exist",
        err.to_string().as_str()
    );
}

#[test]
fn nonexistent_folder() {
    let result = get_config(Some(PathBuf::from("./tests/files/nonexistent/")), None);

    let err = assert_err!(result);
    assert_eq!(
        "Path ./tests/files/nonexistent/ does not exist",
        err.to_string().as_str()
    );
}
