//! CLI command for checking the wash environment and providing recommendations for common issues or missing tools

use crate::cli::{CliCommand, CliContext, CommandOutput};
use anyhow::{Context as _, bail};
use clap::Args;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::{debug, instrument, trace};
use which::which;

#[derive(Debug, Clone, Args)]
pub struct DoctorCommand {}

/// Represents the detected project context
#[derive(Debug, Clone)]
pub enum ProjectContext {
    /// No specific project detected - show all tools
    General,
    /// Rust project (Cargo.toml found)
    Rust { cargo_toml_path: PathBuf },
    /// Go project (go.mod found)
    Go { go_mod_path: PathBuf },
    /// TypeScript/JavaScript project (package.json found)
    TypeScript { package_json_path: PathBuf },
    /// Multiple project types detected
    Mixed { detected_types: Vec<String> },
}

impl CliCommand for DoctorCommand {
    #[instrument(level = "debug", skip_all, name = "doctor")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        let data_dir = ctx.data_dir();
        let cache_dir = ctx.cache_dir();
        let config_dir = ctx.config_dir();
        let config_path = ctx.user_config_path();

        // Determine the directory to check for project context
        let project_dir = ctx.project_dir();
        let project_config = ctx.project_config_path();

        trace!(directory = ?project_dir, "detecting project context");
        let project_context = detect_project_context(project_dir).await?;
        debug!(context = ?project_context, "detected project context");

        // Check for issues to provide recommendations
        let mut issues = Vec::new();
        let mut recommendations = Vec::new();

        // Check configuration directories
        if !data_dir.exists() {
            issues.push("Data directory not found");
            recommendations.push("• Data directory will be created automatically when needed");
        }
        if !cache_dir.exists() {
            issues.push("Cache directory not found");
            recommendations.push("• Cache directory will be created automatically when needed");
        }
        if !config_dir.exists() {
            issues.push("Config directory not found");
            recommendations.push("• Config directory will be created automatically when needed");
        }
        if !config_path.exists() {
            recommendations
                .push("• (Optional) 'wash config init --global' to create a default configuration");
        }
        if !project_config.exists() {
            recommendations
                .push("• (Optional) 'wash config init' to create a default project configuration");
        }

        // Check required binaries
        if !binary_exists_in_path("git") {
            issues.push("Git not found");
            recommendations.push("• Install Git: https://git-scm.com/downloads");
            recommendations.push("  - macOS: 'brew install git' or 'xcode-select --install'");
            recommendations.push("  - Ubuntu/Debian: 'sudo apt-get install git'");
            recommendations.push("  - Windows: Download from https://git-scm.com/");
        }

        // Perform context-specific checks
        let (project_issues, project_recommendations) =
            check_project_specific_tools(&project_context).await?;

        // Add issues and recommendations from project-specific checks
        issues.extend(project_issues);
        recommendations.extend(project_recommendations);

        // Build output based on context
        let mut output_lines = vec![
            "Configuration directories:".to_string(),
            format!(
                "  Data dir: {} {}",
                data_dir.display(),
                if data_dir.exists() {
                    "✅"
                } else {
                    "🟨 not found"
                }
            ),
            format!(
                "  Cache dir: {} {}",
                cache_dir.display(),
                if cache_dir.exists() {
                    "✅"
                } else {
                    "🟨 not found"
                }
            ),
            format!(
                "  Config dir: {} {}",
                config_dir.display(),
                if config_dir.exists() {
                    "✅"
                } else {
                    "🟨 not found"
                }
            ),
            format!(
                "  User Config path: {} {}",
                config_path.display(),
                if config_path.exists() {
                    "✅"
                } else {
                    "🟨 not found"
                }
            ),
            format!(
                "  Project Config path: {} {}",
                config_path.display(),
                if config_path.exists() {
                    "✅"
                } else {
                    "🟨 not found"
                }
            ),
            "".to_string(),
        ];

        // Add project context information
        match &project_context {
            ProjectContext::General => {
                output_lines
                    .push("Project context: General (no specific project detected)".to_string());
            }
            ProjectContext::Rust { cargo_toml_path } => {
                output_lines.push(format!(
                    "Project context: Rust project ({})",
                    cargo_toml_path.display()
                ));
            }
            ProjectContext::Go { go_mod_path } => {
                output_lines.push(format!(
                    "Project context: Go project ({})",
                    go_mod_path.display()
                ));
            }
            ProjectContext::TypeScript { package_json_path } => {
                output_lines.push(format!(
                    "Project context: TypeScript/JavaScript project ({})",
                    package_json_path.display()
                ));
            }
            ProjectContext::Mixed { detected_types } => {
                output_lines.push(format!(
                    "Project context: Mixed project ({})",
                    detected_types.join(", ")
                ));
            }
        }
        output_lines.push("".to_string());

        // Add common tools
        output_lines.extend([
            "Common tools:".to_string(),
            format!(
                "  git:   {}",
                if binary_exists_in_path("git") {
                    "✅ installed"
                } else {
                    "❌ not found"
                }
            ),
            "".to_string(),
        ]);

        // Add context-specific tool sections
        self.add_context_specific_output(&project_context, &mut output_lines)
            .await;

        // Add recommendations section if there are any issues
        if !recommendations.is_empty() {
            output_lines.extend([
                "".to_string(),
                "📋 Recommendations and Fixes:".to_string(),
                "".to_string(),
            ]);
            output_lines.extend(recommendations.into_iter().map(|s| s.to_string()));

            output_lines.extend([
                "".to_string(),
                "💡 Notes:".to_string(),
                "• ✅ = Available and working".to_string(),
                "• 🟨 = Optional or will be created automatically".to_string(),
                "• ❌ = Required for wash functionality".to_string(),
            ]);
        } else {
            output_lines.extend([
                "".to_string(),
                "🎉 All checks passed! Your wash environment is ready.".to_string(),
            ]);
        }

        // Build JSON data for programmatic access
        let mut json_data = serde_json::json!({
            "status": if issues.is_empty() { "healthy" } else { "issues_found" },
            "issues": issues,
            "project_context": match &project_context {
                ProjectContext::General => "general",
                ProjectContext::Rust { .. } => "rust",
                ProjectContext::Go { .. } => "go",
                ProjectContext::TypeScript { .. } => "typescript",
                ProjectContext::Mixed { .. } => "mixed",
            },
            "config": {
                "cache_dir_exists": cache_dir.exists(),
                "config_dir_exists": config_dir.exists(),
                "config_path_exists": config_path.exists(),
                "data_dir_exists": data_dir.exists(),
                "data_dir": data_dir,
                "cache_dir": cache_dir,
                "config_dir": config_dir,
                "config_path": config_path,
            },
        });

        // Add context-specific binary data
        self.add_context_specific_json(&project_context, &mut json_data);

        Ok(CommandOutput::ok(output_lines.join("\n"), Some(json_data)))
    }
}

impl DoctorCommand {
    /// Add context-specific output to the display
    async fn add_context_specific_output(
        &self,
        context: &ProjectContext,
        output_lines: &mut Vec<String>,
    ) {
        match context {
            ProjectContext::General => {
                // Show all tool categories for general context
                output_lines.extend([
                    "Rust tools:".to_string(),
                    format!("  cargo: {status}", status = tool_status("cargo")),
                    "".to_string(),
                    "Go tools:".to_string(),
                    format!("  go: {status}", status = tool_status("go")),
                    "".to_string(),
                    "TypeScript tools:".to_string(),
                    format!("  node: {status}", status = tool_status("node")),
                    format!("  npm: {status}", status = tool_status("npm")),
                ]);
            }
            ProjectContext::Rust { .. } => {
                output_lines.extend([
                    "Rust WebAssembly tools:".to_string(),
                    format!("  cargo: {status}", status = tool_status("cargo")),
                ]);

                // Check wasm32-wasip2 target asynchronously for display
                if binary_exists_in_path("cargo") {
                    let target_status = match which("rustup") {
                        Ok(rustup_path) => {
                            match Command::new(rustup_path)
                                .args(["target", "list", "--installed"])
                                .output()
                                .await
                            {
                                Ok(output) if output.status.success() => {
                                    let stdout = String::from_utf8_lossy(&output.stdout);
                                    if stdout.lines().any(|line| line.trim() == "wasm32-wasip2") {
                                        "✅ installed"
                                    } else {
                                        "❌ not found"
                                    }
                                }
                                _ => "🟨 check failed",
                            }
                        }
                        Err(_) => "🟨 rustup not found",
                    };
                    output_lines.push(format!("  wasm32-wasip2: {target_status}"));
                }
            }
            ProjectContext::Go { .. } => {
                output_lines.extend([
                    "Go WebAssembly tools:".to_string(),
                    format!("  go: {status}", status = tool_status("go")),
                    format!("  tinygo: {status}", status = tool_status("tinygo")),
                    format!("  wasm-opt: {status}", status = tool_status("wasm-opt")),
                    format!("  wasm-tools: {status}", status = tool_status("wasm-tools")),
                ]);
            }
            ProjectContext::TypeScript { .. } => {
                output_lines.extend([
                    "TypeScript/JavaScript tools:".to_string(),
                    format!("  node: {status}", status = tool_status("node")),
                    format!("  npm: {status}", status = tool_status("npm")),
                    format!("  tsc: {status}", status = tool_status("tsc")),
                ]);
            }
            ProjectContext::Mixed { detected_types } => {
                for project_type in detected_types {
                    match project_type.as_str() {
                        "Rust" => {
                            output_lines.extend([
                                "Rust tools:".to_string(),
                                format!("  cargo: {status}", status = tool_status("cargo")),
                                "".to_string(),
                            ]);
                        }
                        "Go" => {
                            output_lines.extend([
                                "Go tools:".to_string(),
                                format!("  go: {status}", status = tool_status("go")),
                                format!("  tinygo: {status}", status = tool_status("tinygo")),
                                "".to_string(),
                            ]);
                        }
                        "TypeScript/JavaScript" => {
                            output_lines.extend([
                                "TypeScript/JavaScript tools:".to_string(),
                                format!("  node: {status}", status = tool_status("node")),
                                format!("  npm: {status}", status = tool_status("npm")),
                                "".to_string(),
                            ]);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Add context-specific data to JSON output
    fn add_context_specific_json(
        &self,
        context: &ProjectContext,
        json_data: &mut serde_json::Value,
    ) {
        let mut binaries = serde_json::Map::new();
        binaries.insert(
            "git".to_string(),
            serde_json::Value::Bool(binary_exists_in_path("git")),
        );

        match context {
            ProjectContext::General => {
                // Include all tools for general context
                binaries.insert(
                    "cargo".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("cargo")),
                );
                binaries.insert(
                    "go".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("go")),
                );
                binaries.insert(
                    "node".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("node")),
                );
                binaries.insert(
                    "npm".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("npm")),
                );
                binaries.insert(
                    "tsc".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("tsc")),
                );
            }
            ProjectContext::Rust { .. } => {
                binaries.insert(
                    "cargo".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("cargo")),
                );
                // Note: wasm32-wasip2 target check would require async, so we'll skip it in JSON for now
            }
            ProjectContext::Go { .. } => {
                binaries.insert(
                    "go".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("go")),
                );
                binaries.insert(
                    "tinygo".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("tinygo")),
                );
                binaries.insert(
                    "wasm-opt".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("wasm-opt")),
                );
                binaries.insert(
                    "wasm-tools".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("wasm-tools")),
                );
            }
            ProjectContext::TypeScript { .. } => {
                binaries.insert(
                    "node".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("node")),
                );
                binaries.insert(
                    "npm".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("npm")),
                );
                binaries.insert(
                    "tsc".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("tsc")),
                );
            }
            ProjectContext::Mixed { .. } => {
                // Include all potentially relevant tools
                binaries.insert(
                    "cargo".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("cargo")),
                );
                binaries.insert(
                    "go".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("go")),
                );
                binaries.insert(
                    "tinygo".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("tinygo")),
                );
                binaries.insert(
                    "node".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("node")),
                );
                binaries.insert(
                    "npm".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("npm")),
                );
                binaries.insert(
                    "wasm-opt".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("wasm-opt")),
                );
                binaries.insert(
                    "wasm-tools".to_string(),
                    serde_json::Value::Bool(binary_exists_in_path("wasm-tools")),
                );
            }
        }

        json_data["binaries"] = serde_json::Value::Object(binaries);
    }
}

/// Perform project-specific tool checks for a given project context
///
/// This function can be called without constructing a DoctorCommand instance,
/// making it useful for other parts of the codebase that need to check tools.
pub async fn check_project_specific_tools(
    context: &ProjectContext,
) -> anyhow::Result<(Vec<&str>, Vec<&str>)> {
    let mut issues = Vec::new();
    let mut recommendations = Vec::new();
    match context {
        ProjectContext::General => {
            // For general context, check all optional tools
            check_optional_tools(&mut issues, &mut recommendations);
        }
        ProjectContext::Rust { .. } => {
            check_rust_tools(&mut issues, &mut recommendations).await?;
        }
        ProjectContext::Go { .. } => {
            check_go_tools(&mut issues, &mut recommendations);
        }
        ProjectContext::TypeScript { .. } => {
            check_typescript_tools(&mut issues, &mut recommendations);
        }
        ProjectContext::Mixed { detected_types } => {
            // Check tools for all detected project types
            if detected_types.iter().any(|t| t == "Rust") {
                check_rust_tools(&mut issues, &mut recommendations).await?;
            }
            if detected_types.iter().any(|t| t == "Go") {
                check_go_tools(&mut issues, &mut recommendations);
            }
            if detected_types.iter().any(|t| t == "TypeScript/JavaScript") {
                check_typescript_tools(&mut issues, &mut recommendations);
            }
        }
    }
    Ok((issues, recommendations))
}

/// Detect the project context based on files in the given directory
///
/// This function can be called without constructing a DoctorCommand instance,
/// making it useful for other parts of the codebase that need to detect project type.
pub async fn detect_project_context(dir: &Path) -> anyhow::Result<ProjectContext> {
    let mut detected_types = Vec::new();
    let mut cargo_toml_path = None;
    let mut go_mod_path = None;
    let mut package_json_path = None;

    // Check for Cargo.toml (Rust project)
    let cargo_toml = dir.join("Cargo.toml");
    if cargo_toml.exists() {
        detected_types.push("Rust".to_string());
        cargo_toml_path = Some(cargo_toml);
    }

    // Check for go.mod (Go project)
    let go_mod = dir.join("go.mod");
    if go_mod.exists() {
        detected_types.push("Go".to_string());
        go_mod_path = Some(go_mod);
    }

    // Check for package.json (TypeScript/JavaScript project)
    let package_json = dir.join("package.json");
    if package_json.exists() {
        detected_types.push("TypeScript/JavaScript".to_string());
        package_json_path = Some(package_json);
    }

    // Return appropriate context based on what was detected
    match detected_types.len() {
        0 => Ok(ProjectContext::General),
        1 => {
            if let Some(path) = cargo_toml_path {
                Ok(ProjectContext::Rust {
                    cargo_toml_path: path,
                })
            } else if let Some(path) = go_mod_path {
                Ok(ProjectContext::Go { go_mod_path: path })
            } else if let Some(path) = package_json_path {
                Ok(ProjectContext::TypeScript {
                    package_json_path: path,
                })
            } else {
                Ok(ProjectContext::General)
            }
        }
        _ => Ok(ProjectContext::Mixed { detected_types }),
    }
}

/// Check tools needed for Rust WebAssembly development
async fn check_rust_tools(
    issues: &mut Vec<&str>,
    recommendations: &mut Vec<&str>,
) -> anyhow::Result<()> {
    // Check for cargo
    if !binary_exists_in_path("cargo") {
        issues.push("Cargo not found (required for Rust development)");
        recommendations.push("• Install Rust and Cargo: https://rustup.rs/");
        recommendations
            .push("  - Run: 'curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh'");
        return Ok(()); // If cargo is missing, can't check targets
    }

    // Check for wasm32-wasip2 target
    match check_rust_target("wasm32-wasip2").await {
        Ok(false) => {
            issues.push("wasm32-wasip2 target not installed");
            recommendations
                .push("• Install wasm32-wasip2 target: 'rustup target add wasm32-wasip2'");
        }
        Err(e) => {
            debug!(error = ?e, "failed to check wasm32-wasip2 target");
            issues.push("Unable to check wasm32-wasip2 target");
            recommendations
                .push("• Check Rust installation and try: 'rustup target add wasm32-wasip2'");
        }
        Ok(true) => {} // Target is installed
    }

    Ok(())
}

/// Check tools needed for Go WebAssembly development  
fn check_go_tools(issues: &mut Vec<&str>, recommendations: &mut Vec<&str>) {
    // Check for go
    if !binary_exists_in_path("go") {
        issues.push("Go not found (required for Go development)");
        recommendations.push("• Install Go: https://golang.org/dl/");
        recommendations.push("  - macOS: 'brew install go'");
        recommendations.push("  - Ubuntu/Debian: 'sudo apt-get install golang-go'");
    }

    // Check for tinygo
    if !binary_exists_in_path("tinygo") {
        issues.push("TinyGo not found (required for Go WebAssembly)");
        recommendations.push("• Install TinyGo: https://tinygo.org/getting-started/install/");
        recommendations.push("  - macOS: 'brew tap tinygo-org/tools && brew install tinygo'");
    } else {
        // Check TinyGo version - simplified for now
        issues.push("TinyGo version check not implemented yet");
        recommendations.push("• Ensure TinyGo >= 0.36.0 is installed");
    }

    // Check for wasm-opt
    if !binary_exists_in_path("wasm-opt") {
        issues.push("wasm-opt not found (recommended for Go WebAssembly)");
        recommendations
            .push("• Install wasm-opt (from binaryen): https://github.com/WebAssembly/binaryen");
        recommendations.push("  - macOS: 'brew install binaryen'");
        recommendations.push("  - Ubuntu/Debian: 'sudo apt-get install binaryen'");
    }

    // Check for wasm-tools
    if !binary_exists_in_path("wasm-tools") {
        issues.push("wasm-tools not found (recommended for Go WebAssembly)");
        recommendations.push("• Install wasm-tools: 'cargo install wasm-tools'");
    }
}

/// Check tools needed for TypeScript/JavaScript development
fn check_typescript_tools(issues: &mut Vec<&str>, recommendations: &mut Vec<&str>) {
    // Check for node
    if !binary_exists_in_path("node") {
        issues.push("Node.js not found (required for TypeScript development)");
        recommendations.push("• Install Node.js: https://nodejs.org/");
        recommendations.push("  - macOS: 'brew install node'");
        recommendations.push("  - Ubuntu/Debian: 'sudo apt-get install nodejs npm'");
        return; // If node is missing, npm will also be missing
    }

    // Check for npm
    if !binary_exists_in_path("npm") {
        issues.push("npm not found (should come with Node.js)");
        recommendations.push("• npm should be included with Node.js installation");
        recommendations.push("• Try reinstalling Node.js from https://nodejs.org/");
    }
}

/// Check all optional tools for general context
fn check_optional_tools(_issues: &mut [&str], recommendations: &mut Vec<&str>) {
    if !binary_exists_in_path("cargo") {
        recommendations.push("• Install Rust and Cargo for Rust development: https://rustup.rs/");
    }

    if !binary_exists_in_path("go") {
        recommendations.push("• Install Go for Go development: https://golang.org/dl/");
    }

    if !binary_exists_in_path("node") {
        recommendations
            .push("• Install Node.js for TypeScript/JavaScript development: https://nodejs.org/");
    }
}

/// Check if a Rust target is installed
async fn check_rust_target(target: &str) -> anyhow::Result<bool> {
    let rustup_path = which("rustup").context("rustup not found in PATH")?;

    let output = Command::new(rustup_path)
        .args(["target", "list", "--installed"])
        .output()
        .await
        .context("failed to run rustup target list")?;

    if !output.status.success() {
        bail!("rustup target list failed");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().any(|line| line.trim() == target))
}

/// Helper function to check if a binary exists in the system's PATH
pub fn binary_exists_in_path(binary_name: &str) -> bool {
    which(binary_name).is_ok()
}

/// Helper function to get the full path of a binary in the system's PATH
pub fn get_binary_path(binary_name: &str) -> Option<PathBuf> {
    which(binary_name).ok()
}

/// Helper function to get tool status string
fn tool_status(tool: &str) -> &'static str {
    if binary_exists_in_path(tool) {
        "✅ installed"
    } else {
        "🟨 not found"
    }
}

// Example usage of the public functions for external callers:
//
// ```rust,ignore
// use crate::cli::doctor::{detect_project_context, check_project_specific_tools, binary_exists_in_path};
// use std::path::Path;
//
// async fn check_tools_example() -> anyhow::Result<()> {
//     let project_dir = Path::new(".");
//
//     // Detect what kind of project we're in
//     let context = detect_project_context(project_dir).await?;
//
//     // Check for missing tools
//     let mut issues = Vec::new();
//     let mut recommendations = Vec::new();
//     check_project_specific_tools(&context, &mut issues, &mut recommendations).await?;
//
//     // Check individual binaries
//     if !binary_exists_in_path("cargo") {
//         println!("Cargo is not installed");
//     }
//
//     Ok(())
// }
// ```
