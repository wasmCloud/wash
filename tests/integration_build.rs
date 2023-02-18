use anyhow::Result;

mod common;
use common::wash;
use scopeguard::defer;
use serial_test::serial;
use std::{
    env::temp_dir,
    fs::{create_dir_all, remove_dir_all},
    path::PathBuf,
};

#[test]
#[serial]
fn build_rust_actor_unsigned() -> Result<()> {
    let TestSetup {
        project_dir,
        test_dir,
    } = init(
        /* test_name= */ "build_rust_actor_unsigned",
        /* actor_name= */ "hello",
        /* template_name= */ "actor/hello",
    )?;
    defer! {
        remove_dir_all(test_dir).unwrap();
    }

    let status = wash()
        .args(["build", "--build-only"])
        .status()
        .expect("Failed to build project");

    assert!(status.success());
    let unsigned_file = project_dir.join("build/hello.wasm");
    assert!(unsigned_file.exists(), "unsigned file not found!");
    let signed_file = project_dir.join("build/hello_s.wasm");
    assert!(
        !signed_file.exists(),
        "signed file should not exist when using --build-only!"
    );
    Ok(())
}

#[test]
#[serial]
fn build_rust_actor_signed() -> Result<()> {
    let TestSetup {
        project_dir,
        test_dir,
    } = init(
        /* test_name= */ "build_rust_actor_signed",
        /* actor_name= */ "hello",
        /* template_name= */ "actor/hello",
    )?;
    defer! {
        remove_dir_all(test_dir).unwrap();
    }

    let status = wash()
        .args(["build"])
        .status()
        .expect("Failed to build project");

    assert!(status.success());
    let unsigned_file = project_dir.join("build/hello.wasm");
    assert!(unsigned_file.exists(), "unsigned file not found!");
    let signed_file = project_dir.join("build/hello_s.wasm");
    assert!(signed_file.exists(), "signed file not found!");
    Ok(())
}

#[test]
#[serial]
fn build_tinygo_actor_signed() -> Result<()> {
    let TestSetup {
        project_dir,
        test_dir,
    } = init(
        /* test_name= */ "build_tinygo_actor_signed",
        /* actor_name= */ "echo",
        /* template_name= */ "echo-tinygo",
    )?;
    defer! {
        remove_dir_all(test_dir).unwrap();
    }

    let status = wash()
        .args(["build"])
        .status()
        .expect("Failed to build project");

    assert!(status.success());
    let unsigned_file = project_dir.join("build/echo.wasm");
    assert!(unsigned_file.exists(), "unsigned file not found!");
    let signed_file = project_dir.join("build/echo_s.wasm");
    assert!(signed_file.exists(), "signed file not found!");
    Ok(())
}

struct TestSetup {
    /// The path to the directory for the test.
    test_dir: PathBuf,
    /// The path to the created actor's directory.
    project_dir: PathBuf,
}

/// Inits an actor build test by setting up a test directory and creating an actor from a template.
/// Returns the paths of the test directory and actor directory.
fn init(test_name: &str, actor_name: &str, template_name: &str) -> Result<TestSetup> {
    let test_dir = setup_test_dir(test_name)?;
    let project_dir = init_actor_from_template(actor_name, template_name)?;
    Ok(TestSetup {
        test_dir,
        project_dir,
    })
}

/// Sets up a test directory for the current test, and sets the environment to use that directory.
/// Returns the path to the test directory.
fn setup_test_dir(subfolder: &str) -> Result<PathBuf> {
    let test_dir = temp_dir().join(subfolder);
    if test_dir.exists() {
        remove_dir_all(&test_dir)?;
    }
    create_dir_all(&test_dir)?;
    std::env::set_current_dir(&test_dir)?;

    Ok(test_dir)
}

/// Initializes a new actor from a wasmCloud template, and sets the environment to use the created actor's directory.
fn init_actor_from_template(actor_name: &str, template_name: &str) -> Result<PathBuf> {
    let status = wash()
        .args([
            "new",
            "actor",
            actor_name,
            "--git",
            "wasmcloud/project-templates",
            "--subfolder",
            &format!("actor/{template_name}"),
            "--silent",
            "--no-git-init",
        ])
        .status()
        .expect("Failed to generate project");

    assert!(status.success());

    let project_dir = std::env::current_dir()?.join(actor_name);
    std::env::set_current_dir(&project_dir)?;
    Ok(project_dir)
}
