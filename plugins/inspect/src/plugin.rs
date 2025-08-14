//! Implementation of `wasmcloud:wash/plugin` for the inspect plugin

use base64::Engine;
use crate::bindings::wasi::logging::logging::{log, Level};
use crate::bindings::wasmcloud::wash::types::{
    Command, CommandArgument, HookType, Metadata, Runner,
};
use crate::{inspect_component_bytes, read_file_bytes, file_exists};

impl crate::bindings::exports::wasmcloud::wash::plugin::Guest for crate::Component {
    /// Called by wash to retrieve the plugin metadata
    fn info() -> Metadata {
        Metadata {
            id: "dev.wasmcloud.inspect".to_string(),
            name: "inspect".to_string(),
            description: "Inspect a WebAssembly component and display its WIT interface"
                .to_string(),
            contact: "wasmCloud Team".to_string(),
            url: "https://github.com/wasmcloud/wash".to_string(),
            license: "Apache-2.0".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            command: Some(Command {
                id: "inspect".to_string(),
                name: "inspect".to_string(),
                description: "Inspect a WebAssembly component and display its WIT interface"
                    .to_string(),
                flags: vec![],
                arguments: vec![CommandArgument {
                    name: "component_path".to_string(),
                    description: "Path to the WebAssembly component file to inspect".to_string(),
                    env: None,
                    default: None,
                    value: None,
                }],
                usage: vec![
                    "wash inspect ./my-component.wasm".to_string(),
                    "wash inspect /path/to/component.wasm".to_string(),
                ],
            }),
            sub_commands: vec![],
            hooks: vec![HookType::AfterDev],
        }
    }

    fn initialize(_runner: Runner) -> Result<String, String> {
        log(Level::Info, "", "Inspect plugin initialized successfully");
        Ok("Inspect plugin ready".to_string())
    }

    fn run(runner: Runner, command: Command) -> Result<String, String> {
        log(
            Level::Debug,
            "",
            &format!("Executing inspect command: {}", command.name),
        );

        // Get the component path from the first argument
        let component_path = match command.arguments.first() {
            Some(arg) => match &arg.value {
                Some(path) => path.clone(),
                None => {
                    return Err(
                        "Component path argument is required but was not provided".to_string()
                    )
                }
            },
            None => return Err("Component path argument is required".to_string()),
        };

        match read_file_bytes(&component_path) {
            Ok(component_bytes) => {
                let wit = inspect_component_bytes(&component_bytes).map_err(|e| {
                    format!(
                        "Failed to inspect component at path: {}: {}",
                        component_path, e
                    )
                })?;
                Ok(wit)
            }
            Err(e) => {
                let error_msg =
                    format!("Failed to read component file '{}': {}", component_path, e);
                runner.error(&error_msg);
                Err(error_msg)
            }
        }
    }

    fn hook(runner: Runner, hook: HookType) -> Result<String, String> {
        match hook {
            HookType::AfterDev => {
                log(
                    Level::Info,
                    "",
                    "Executing AfterDev hook - inspecting component after development session",
                );

                // TODO: Get the artifact path from the context
                // The development session should have set the artifact path in the context
                let context = runner
                    .context()
                    .map_err(|e| format!("Failed to get runner context: {}", e))?;

                // Look for component bytes in the context (base64 encoded)
                let component_bytes = match context.get("dev.component_bytes_base64") {
                    Some(b64_data) => {
                        match base64::engine::general_purpose::STANDARD.decode(b64_data) {
                            Ok(bytes) => bytes,
                            Err(e) => {
                                let error_msg = format!(
                                    "Failed to decode component bytes from base64: {}", e
                                );
                                log(Level::Error, "", &error_msg);
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
                                        log(
                                            Level::Info,
                                            "",
                                            &format!(
                                                "No component bytes in context, using fallback: {}",
                                                path
                                            ),
                                        );
                                        found_bytes = Some(bytes);
                                        break;
                                    }
                                    Err(e) => {
                                        log(
                                            Level::Debug,
                                            "",
                                            &format!("Failed to read {}: {}", path, e),
                                        );
                                    }
                                }
                            }
                        }

                        match found_bytes {
                            Some(bytes) => bytes,
                            None => {
                                let msg = "No component artifact found after development session. \
                                          Expected to find component bytes in context or at common locations.";
                                log(Level::Warn, "", msg);
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
                        let error_msg = format!(
                            "Failed to inspect component after dev session: {}", e
                        );
                        Err(error_msg)
                    }
                }
            }
            _ => {
                let msg = format!(
                    "Hook type {:?} is not supported by the inspect plugin",
                    hook
                );
                log(Level::Warn, "", &msg);
                Err(msg)
            }
        }
    }
}
