//! Implementation of `wasmcloud:wash/plugin` for the inspect plugin

// Removed wasi-logging - using stdout/stderr directly
use crate::bindings::wasmcloud::wash::types::{
    Command, CommandArgument, HookType, Metadata, Runner,
};
use crate::{file_exists, inspect_component_bytes, read_file_bytes};
use base64::Engine;

/// Check if a path is a directory using wasi:filesystem
fn is_directory(path: &str) -> anyhow::Result<bool> {
    let (root_dir, _root_path) = crate::get_root_dir()
        .map_err(|e| anyhow::anyhow!("Failed to get root directory: {}", e))?;

    match root_dir.stat_at(wasi::filesystem::types::PathFlags::empty(), path) {
        Ok(stat) => match stat.type_ {
            wasi::filesystem::types::DescriptorType::Directory => Ok(true),
            _ => Ok(false),
        },
        Err(e) => {
            if e == wasi::filesystem::types::ErrorCode::NoEntry {
                Ok(false)
            } else {
                anyhow::bail!("Failed to stat path {}: {}", path, e)
            }
        }
    }
}

/// Check if a directory contains project files (Cargo.toml, go.mod, package.json, etc.)
fn is_project_directory(path: &str) -> anyhow::Result<bool> {
    let project_files = [
        "Cargo.toml",
        "go.mod",
        "package.json",
        "wasmcloud.toml",
        ".wash/config.yaml",
    ];

    for file in &project_files {
        let project_file_path = if path == "." {
            file.to_string()
        } else {
            format!("{}/{}", path, file)
        };

        if file_exists(&project_file_path).unwrap_or(false) {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Find common component component paths relative to a project directory
fn find_component_artifact(project_path: &str) -> anyhow::Result<Option<String>> {
    let common_component_paths = [
        "target/wasm32-wasip2/release/component.wasm",
        "target/wasm32-wasip2/debug/component.wasm",
        "build/component.wasm",
        "build/output.wasm",
        "dist/component.wasm",
        "component.wasm",
    ];

    for component_path in &common_component_paths {
        let full_path = if project_path == "." {
            component_path.to_string()
        } else {
            format!("{}/{}", project_path, component_path)
        };

        if file_exists(&full_path).unwrap_or(false) {
            return Ok(Some(full_path));
        }
    }

    Ok(None)
}

/// Request that wash build the project and return the component path
fn request_project_build(runner: &Runner, project_path: &str) -> Result<String, String> {
    println!("Building project at: {}", project_path);

    // Request wash to build the project
    match runner.host_exec("wash", &["build".to_string(), project_path.to_string()]) {
        Ok((stdout, stderr)) => {
            // Try to find the built artifact
            match find_component_artifact(project_path) {
                Ok(Some(component_path)) => Ok(component_path),
                Ok(None) => Err(format!(
                    "Build completed but no component artifact found in project: {}",
                    project_path
                )),
                Err(e) => Err(format!(
                    "Failed to find component artifact after build: {}",
                    e
                )),
            }
        }
        Err(e) => Err(format!(
            "Failed to build project at {}: {}",
            project_path, e
        )),
    }
}

impl crate::bindings::exports::wasmcloud::wash::plugin::Guest for crate::Component {
    /// Called by wash to retrieve the plugin metadata
    fn info() -> Metadata {
        Metadata {
            id: "dev.wasmcloud.inspect".to_string(),
            name: "inspect-enhanced".to_string(),
            description: "Inspect a WebAssembly component and display its WIT interface"
                .to_string(),
            contact: "wasmCloud Team".to_string(),
            url: "https://github.com/wasmcloud/wash".to_string(),
            license: "Apache-2.0".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            command: Some(Command {
                id: "inspect-enhanced".to_string(),
                name: "inspect-enhanced".to_string(),
                description: "Inspect a WebAssembly component and display its WIT interface"
                    .to_string(),
                flags: vec![],
                arguments: vec![CommandArgument {
                    name: "component_path".to_string(),
                    description: "Path to the WebAssembly component file or project directory to inspect. If omitted, inspects the current directory as a project.".to_string(),
                    env: None,
                    default: Some(".".to_string()),
                    value: None,
                }],
                usage: vec![
                    "wash inspect-enhanced".to_string(),
                    "wash inspect-enhanced ./my-component.wasm".to_string(),
                    "wash inspect-enhanced /path/to/component.wasm".to_string(),
                    "wash inspect-enhanced ./my-project/".to_string(),
                ],
            }),
            sub_commands: vec![],
            hooks: vec![HookType::AfterDev],
        }
    }

    fn initialize(_runner: Runner) -> Result<String, String> {
        println!("Inspect plugin initialized successfully");
        Ok("Inspect plugin ready".to_string())
    }

    fn run(runner: Runner, command: Command) -> Result<String, String> {
        // Get the component path from the first argument, defaulting to "." (current directory)
        let component_path = match command.arguments.first() {
            // Some(CommandArgumentValue::Path(path)) => match &arg.value {
            Some(arg) => match &arg.value {
                Some(path) => path.clone(),
                None => match &arg.default {
                    Some(default) => default.clone(),
                    None => ".".to_string(),
                },
            },
            None => ".".to_string(),
        };

        // Determine the actual component file to inspect
        let actual_component_path = if component_path.ends_with(".wasm") {
            // Direct wasm file path - use as-is
            component_path
        } else {
            // Could be a directory or non-wasm file
            match is_directory(&component_path) {
                Ok(true) => {
                    // It's a directory - check if it's a project

                    match is_project_directory(&component_path) {
                        Ok(true) => {
                            println!("Detected project directory: {}", component_path);

                            // Try to find existing artifact first
                            match find_component_artifact(&component_path) {
                                Ok(Some(component_path)) => {
                                    println!("Found existing artifact: {}", component_path);
                                    component_path
                                }
                                Ok(None) => {
                                    // No existing artifact - build the project
                                    println!("No existing artifact found, building project");
                                    match request_project_build(&runner, &component_path) {
                                        Ok(built_path) => {
                                            println!("Built component: {}", built_path);
                                            built_path
                                        }
                                        Err(e) => {
                                            return Err(format!("Failed to build project: {}", e));
                                        }
                                    }
                                }
                                Err(e) => {
                                    return Err(format!(
                                        "Failed to search for component artifact: {}",
                                        e
                                    ));
                                }
                            }
                        }
                        Ok(false) => {
                            return Err(format!(
                                "Directory {} does not appear to be a project (no Cargo.toml, go.mod, package.json, or .wash/config.yaml found)",
                                component_path
                            ));
                        }
                        Err(e) => {
                            return Err(format!(
                                "Failed to check if {} is a project directory: {}",
                                component_path, e
                            ));
                        }
                    }
                }
                Ok(false) => {
                    // Not a directory - treat as file path
                    component_path
                }
                Err(e) => {
                    return Err(format!(
                        "Failed to check if {} is a directory: {}",
                        component_path, e
                    ));
                }
            }
        };

        println!("Inspecting component: {}", actual_component_path);

        // Read and inspect the component
        match read_file_bytes(&actual_component_path) {
            Ok(component_bytes) => {
                let wit = inspect_component_bytes(&component_bytes).map_err(|e| {
                    format!(
                        "Failed to inspect component at path: {}: {}",
                        actual_component_path, e
                    )
                })?;

                let result = format!(
                    "🔍 Component inspection: {}\n\nWIT Interface:\n{}",
                    actual_component_path, wit
                );
                Ok(result)
            }
            Err(e) => {
                let error_msg = format!(
                    "Failed to read component file '{}': {}",
                    actual_component_path, e
                );
                eprintln!("{}", error_msg);
                Err(error_msg)
            }
        }
    }

    fn hook(runner: Runner, hook: HookType) -> Result<String, String> {
        match hook {
            HookType::AfterDev => {
                println!(
                    "Executing AfterDev hook - inspecting component after development session"
                );

                // TODO: Get the component path from the context
                // The development session should have set the component path in the context
                let context = runner
                    .context()
                    .map_err(|e| format!("Failed to get runner context: {}", e))?;

                // Look for component bytes in the context (base64 encoded)
                let component_bytes = match context.get("dev.component_bytes_base64") {
                    Some(b64_data) => {
                        match base64::engine::general_purpose::STANDARD.decode(b64_data) {
                            Ok(bytes) => bytes,
                            Err(e) => {
                                let error_msg =
                                    format!("Failed to decode component bytes from base64: {}", e);
                                eprintln!("{}", error_msg);
                                return Err(error_msg);
                            }
                        }
                    }
                    None => {
                        // Fallback: try to read from common artifact locations
                        let common_paths = [
                            "./target/wasm32-wasip2/release/component.wasm",
                            "./build/component.wasm",
                            "./dist/component.wasm",
                            "./component.wasm",
                        ];

                        let mut found_bytes = None;
                        for path in &common_paths {
                            if file_exists(path).unwrap_or(false) {
                                match read_file_bytes(path) {
                                    Ok(bytes) => {
                                        println!(
                                            "No component bytes in context, using fallback: {}",
                                            path
                                        );
                                        found_bytes = Some(bytes);
                                        break;
                                    }
                                    Err(e) => {}
                                }
                            }
                        }

                        match found_bytes {
                            Some(bytes) => bytes,
                            None => {
                                let msg = "No component artifact found after development session. \
                                          Expected to find component bytes in context or at common locations.";
                                eprintln!("{}", msg);
                                return Ok(msg.to_string());
                            }
                        }
                    }
                };

                // Inspect the component bytes
                match inspect_component_bytes(&component_bytes) {
                    Ok(wit) => {
                        let message = format!(
                            "🔍 Component inspection complete after development session\n\
                             \n\
                             WIT Interface:\n\
                             {}\n",
                            wit
                        );
                        Ok(message)
                    }
                    Err(e) => {
                        let error_msg =
                            format!("Failed to inspect component after dev session: {}", e);
                        Err(error_msg)
                    }
                }
            }
            _ => {
                let msg = format!(
                    "Hook type {:?} is not supported by the inspect plugin",
                    hook
                );
                eprintln!("{}", msg);
                Err(msg)
            }
        }
    }
}
