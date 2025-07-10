//! CLI command for building components, including Rust, TinyGo, and TypeScript projects

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, bail};
use clap::Args;
use etcetera::AppStrategy;
use serde::Serialize;
use tracing::{debug, error, info, instrument, trace, warn};

use crate::component_build::ProjectType;
use crate::{
    cli::{CliContext, CommandOutput},
    config::{Config, generate_project_config, load_config},
    wit::{CommonPackageArgs, WkgFetcher, load_lock_file},
};

/// CLI command for building components
#[derive(Debug, Clone, Args, Serialize)]
pub struct ComponentBuildCommand {
    /// Path to the project directory
    #[clap(name = "project-path", default_value = ".")]
    project_path: PathBuf,

    /// Path to a configuration file in a location other than PROJECT_DIR/.wash/config.json
    #[clap(name = "config", long = "config")]
    build_config: Option<PathBuf>,

    /// The expected path to the built Wasm component artifact
    #[clap(long = "artifact-path")]
    artifact_path: Option<PathBuf>,

    /// Skip fetching WIT dependencies, useful for offline builds
    #[clap(long = "skip-fetch")]
    skip_fetch: bool,
}

impl ComponentBuildCommand {
    #[instrument(level = "debug", skip(self, ctx), name = "component_build")]
    pub async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        // Load configuration with CLI arguments override
        let config = load_config(&ctx.config_path(), Some(&self.project_path), Some(self))?;

        let result = build_component(&self.project_path, ctx, &config).await?;

        Ok(CommandOutput::ok(
            format!(
                "Successfully built component at: {}",
                result.artifact_path.display()
            ),
            Some(serde_json::json!({
                "artifact_path": result.artifact_path,
                "project_type": result.project_type,
                "project_path": self.project_path,
            })),
        ))
    }
}

impl From<&ComponentBuildCommand> for Config {
    fn from(cmd: &ComponentBuildCommand) -> Self {
        Config {
            wit: Some(crate::wit::WitConfig {
                skip_fetch: cmd.skip_fetch,
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

/// Result of a component build operation
#[derive(Debug, Clone)]
pub struct ComponentBuildResult {
    /// Path to the built component artifact
    pub artifact_path: PathBuf,
    /// Type of project that was built
    pub project_type: ProjectType,
    /// Original project path
    pub project_path: PathBuf,
}

/// Build a component at the specified project path
///
/// This is the main public interface for building components that can be reused
/// throughout the project. It handles project detection, tool validation, and
/// the actual build process.
#[instrument(level = "debug", skip(ctx, config), name = "build_component")]
pub async fn build_component(
    project_path: &Path,
    ctx: &CliContext,
    config: &Config,
) -> anyhow::Result<ComponentBuildResult> {
    let skip_fetch = config.wit.as_ref().map(|w| w.skip_fetch).unwrap_or(false);
    let builder = ComponentBuilder::new(project_path.to_path_buf(), None, skip_fetch);
    builder.build(ctx, config).await
}

/// Component builder that handles the actual build process
#[derive(Debug, Clone)]
pub struct ComponentBuilder {
    project_path: PathBuf,
    wit_dir: Option<PathBuf>,
    skip_wit_fetch: bool,
}

impl ComponentBuilder {
    /// Create a new component builder for the specified project path
    pub fn new(project_path: PathBuf, wit_dir: Option<PathBuf>, skip_wit_fetch: bool) -> Self {
        Self {
            project_path,
            wit_dir,
            skip_wit_fetch,
        }
    }

    /// Get the WIT directory, defaulting to project_path/wit if not specified
    fn get_wit_dir(&self) -> PathBuf {
        self.wit_dir
            .clone()
            .unwrap_or_else(|| self.project_path.join("wit"))
    }

    /// Build the component
    #[instrument(level = "debug", skip(self, ctx, config))]
    pub async fn build(
        &self,
        ctx: &CliContext,
        config: &Config,
    ) -> anyhow::Result<ComponentBuildResult> {
        info!(
            path = ?self.project_path.display(),
            "building component project",
        );

        // Validate project path exists
        if !self.project_path.exists() {
            bail!(
                "project path does not exist: {}",
                self.project_path.display()
            );
        }

        // Detect project language
        let project_type = self.detect_project_type().await?;
        debug!(?project_type, "detected project type");

        // Check for required tools based on project type
        self.check_required_tools(&project_type)?;

        // Fetch WIT dependencies if needed
        if !self.skip_wit_fetch {
            debug!("fetching WIT dependencies for project");
            if let Err(e) = self.fetch_wit_dependencies(ctx, config).await {
                error!(err = ?e, "unable to fetch WIT dependencies. If dependencies are already present locally, you can skip this step with --skip-fetch");
                bail!(e);
            }
        } else {
            debug!("skipping WIT dependency fetching as per configuration");
        }

        // Run pre-build hook
        self.run_pre_build_hook().await?;

        info!(path = ?self.project_path.display(), "starting component build");
        // Build the component using the language toolchain
        let artifact_path = match project_type {
            ProjectType::Rust => self.build_rust_component(config).await?,
            ProjectType::Go => self.build_tinygo_component(config).await?,
            ProjectType::TypeScript => self.build_typescript_component(config).await?,
            ProjectType::Unknown => {
                bail!("unknown project type. Expected to find Cargo.toml, go.mod, or package.json");
            }
        };

        // Run post-build hook
        self.run_post_build_hook().await?;

        // Write project configuration
        generate_project_config(&self.project_path, &project_type, config)?;

        // Attempt to canonicalize the artifact path
        let artifact_path = artifact_path.canonicalize().unwrap_or(artifact_path);

        Ok(ComponentBuildResult {
            artifact_path,
            project_type,
            project_path: self.project_path.clone(),
        })
    }

    /// Detect the project type based on files in the project directory
    async fn detect_project_type(&self) -> anyhow::Result<ProjectType> {
        // Check for Cargo.toml (Rust project)
        if self.project_path.join("Cargo.toml").exists() {
            return Ok(ProjectType::Rust);
        }

        // Check for go.mod (Go project)
        if self.project_path.join("go.mod").exists() {
            return Ok(ProjectType::Go);
        }

        // Check for package.json (TypeScript/JavaScript project)
        if self.project_path.join("package.json").exists() {
            return Ok(ProjectType::TypeScript);
        }

        Ok(ProjectType::Unknown)
    }

    /// Check for required tools based on project type
    fn check_required_tools(&self, project_type: &ProjectType) -> anyhow::Result<()> {
        let mut missing_tools = Vec::new();
        let mut warnings = Vec::new();

        match project_type {
            ProjectType::Rust => {
                // Check for cargo
                if !self.tool_exists("cargo", "--version") {
                    missing_tools.push("cargo (Rust build tool)");
                } else {
                    // Check for wasm32-wasip2 target
                    match Command::new("rustup")
                        .args(["target", "list", "--installed"])
                        .output()
                    {
                        Ok(output) => {
                            let installed_targets = String::from_utf8_lossy(&output.stdout);
                            if !installed_targets.contains("wasm32-wasip2") {
                                warnings.push("wasm32-wasip2 target not installed. Run: rustup target add wasm32-wasip2");
                            }
                        }
                        Err(_) => {
                            warnings
                                .push("rustup not found, cannot check for wasm32-wasip2 target");
                        }
                    }
                }
            }
            ProjectType::Go => {
                if !self.tool_exists("go", "version") {
                    missing_tools.push("go (Go compiler)");
                }
                if !self.tool_exists("tinygo", "version") {
                    missing_tools.push("tinygo (TinyGo compiler for WebAssembly)");
                }
                if !self.tool_exists("wasm-tools", "--version") {
                    missing_tools.push("wasm-tools (Wasm tools for Go)");
                }
            }
            ProjectType::TypeScript => {
                if !self.tool_exists("node", "--version") {
                    missing_tools.push("node (Node.js runtime)");
                }
                if !self.tool_exists("npm", "--version") {
                    missing_tools.push("npm (Node.js package manager)");
                }
            }
            ProjectType::Unknown => {
                bail!("cannot check tools for unknown project type");
            }
        }

        // Report warnings
        for warning in warnings {
            warn!(warning = %warning, "⚠️  Warning");
        }

        // Report missing tools
        if !missing_tools.is_empty() {
            error!("missing required tools:");
            for tool in &missing_tools {
                error!(tool = %tool, "  - missing tool");
            }
            bail!(
                "missing required tools: {tools}",
                tools = missing_tools.join(", ")
            );
        }

        Ok(())
    }

    /// Check if a tool exists in PATH, passing it the subcommand `cmd` to check.
    ///
    /// For example, `cargo --version` or `tinygo version`.
    fn tool_exists(&self, tool: &str, cmd: &str) -> bool {
        Command::new(tool)
            .arg(cmd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    /// Fetch WIT dependencies if the project has any
    #[instrument(
        level = "debug",
        skip(self, ctx, _config),
        name = "fetch_wit_dependencies"
    )]
    async fn fetch_wit_dependencies(
        &self,
        ctx: &CliContext,
        _config: &Config,
    ) -> anyhow::Result<()> {
        let wit_dir = self.get_wit_dir();

        // Check if WIT directory exists - if not, skip dependency fetching
        if !wit_dir.exists() {
            debug!(
                "WIT directory does not exist, skipping dependency fetching: {}",
                wit_dir.display()
            );
            return Ok(());
        }

        debug!(path = ?wit_dir.display(), "fetching WIT dependencies");

        // Create WIT fetcher from configuration
        let mut lock_file = load_lock_file(&self.project_path).await?;
        let args = CommonPackageArgs {
            config: None, // TODO(#1): config
            cache: Some(ctx.cache_dir().join("package_cache")),
        };
        let config = wasm_pkg_core::config::Config::default();
        let fetcher = WkgFetcher::from_common(&args, config).await?;

        // Fetch dependencies
        fetcher
            .fetch_wit_dependencies(&wit_dir, &mut lock_file)
            .await?;

        lock_file
            .write()
            .await
            .context("failed to write lock file")?;

        debug!("WIT dependencies fetched successfully");
        Ok(())
    }

    /// Build a Rust component using cargo
    async fn build_rust_component(&self, config: &Config) -> anyhow::Result<PathBuf> {
        debug!("building rust component");

        // Get Rust build configuration, use defaults if not specified
        let rust_config = config
            .build
            .as_ref()
            .and_then(|b| b.rust.as_ref())
            .cloned()
            .unwrap_or_default();

        // Check if custom_command is specified - if so, use it instead of standard build
        if let Some(custom_command) = &rust_config.custom_command {
            return self.execute_custom_command(custom_command).await;
        }

        // Build cargo command arguments
        let mut cargo_args = vec!["build".to_string()];

        // Apply release mode if configured
        if rust_config.release {
            cargo_args.push("--release".to_string());
        }

        // Apply target - now uses explicit default from config
        cargo_args.push("--target".to_string());
        cargo_args.push(rust_config.target.clone());

        // Apply features if configured
        if !rust_config.features.is_empty() {
            cargo_args.push("--features".to_string());
            cargo_args.push(rust_config.features.join(","));
        }

        // Apply no-default-features if configured
        if rust_config.no_default_features {
            cargo_args.push("--no-default-features".to_string());
        }

        // Add any additional cargo flags if configured
        for flag in &rust_config.cargo_flags {
            cargo_args.push(flag.clone());
        }

        debug!(cargo_args = ?cargo_args, "running cargo with args");

        // Change to project directory and run cargo build
        let output = Command::new("cargo")
            .args(&cargo_args)
            .current_dir(&self.project_path)
            .output()
            .context("failed to execute cargo build")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("{stderr}");
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        debug!(stdout = %stdout, "cargo build output");

        if let Some(artifact_path) = config.build.as_ref().and_then(|b| b.artifact_path.as_ref()) {
            if artifact_path.exists() {
                debug!(artifact_path = %artifact_path.display(), "found component artifact at specified path");
                return Ok(artifact_path.to_owned());
            } else if self.project_path.join(artifact_path).exists() {
                let abs_path = self.project_path.join(artifact_path);
                debug!(artifact_path = %abs_path.display(), "found component artifact at specified path relative to project root");
                return Ok(abs_path);
            } else {
                bail!(
                    "specified artifact path does not exist: {}",
                    artifact_path.display()
                )
            }
        }

        // Find the generated wasm file
        let build_type = if rust_config.release {
            "release"
        } else {
            "debug"
        };
        let target_dir = self
            .project_path
            .join(format!("target/{}/{}", rust_config.target, build_type));
        let entries = std::fs::read_dir(&target_dir).context("failed to read target directory")?;

        for entry in entries {
            let entry = entry.context("failed to read directory entry")?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("wasm") {
                debug!(artifact_path = %path.display(), "found component artifact");
                return Ok(path);
            }
        }

        Err(anyhow::anyhow!(
            "no .wasm file found in target directory: {}",
            target_dir.display()
        ))
    }

    /// Build a TinyGo component using tinygo
    async fn build_tinygo_component(&self, config: &Config) -> anyhow::Result<PathBuf> {
        debug!("building tinygo component with tinygo");

        // Get TinyGo build configuration, use defaults if not specified
        let tinygo_config = config
            .build
            .as_ref()
            .and_then(|b| b.tinygo.as_ref())
            .cloned()
            .unwrap_or_default();

        // Check if custom_command is specified - if so, use it instead of standard build
        if let Some(custom_command) = &tinygo_config.custom_command {
            return self.execute_custom_command(custom_command).await;
        }

        // Create build directory if it doesn't exist
        let build_dir = self.project_path.join("build");
        if !build_dir.exists() {
            std::fs::create_dir_all(&build_dir).context("failed to create build directory")?;
        }

        let output_file = build_dir.join("component.wasm");

        // Build tinygo command arguments
        let mut tinygo_args = vec![
            "build".to_string(),
            "-o".to_string(),
            output_file
                .to_str()
                .context("failed to convert output file path to str")?
                .to_string(),
        ];

        // Apply target - now uses explicit default from config
        tinygo_args.push("-target".to_string());
        tinygo_args.push(tinygo_config.target.to_string());

        // Add WIT package and world for component models
        if let Some(wit_package) = &tinygo_config.wit_package {
            tinygo_args.push("-wit-package".to_string());
            tinygo_args.push(wit_package.to_string());
        } else {
            debug!("no WIT package specified, skipping -wit-package argument");
        }

        if let Some(wit_world) = &tinygo_config.wit_world {
            tinygo_args.push("-wit-world".to_string());
            tinygo_args.push(wit_world.to_string());
        } else {
            debug!("no WIT world specified, skipping -wit-world argument");
        }

        // Apply garbage collector - now uses explicit default from config
        tinygo_args.push("-gc".to_string());
        tinygo_args.push(tinygo_config.gc.to_string());

        // Add debug settings - disable by default for smaller builds
        tinygo_args.push("-no-debug".to_string());

        // Add any additional build flags if configured
        for flag in &tinygo_config.build_flags {
            tinygo_args.push(flag.to_string());
        }

        // Add source directory
        tinygo_args.push(".".to_string());

        debug!(tinygo_args = ?tinygo_args, "running tinygo with args");

        // Run tinygo build command
        let output = Command::new("tinygo")
            .args(&tinygo_args)
            .current_dir(&self.project_path)
            .output()
            .context("failed to execute tinygo build")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(stderr = %stderr, "tinygo build failed");
            bail!("TinyGo build failed: {stderr}");
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        debug!(stdout = %stdout, "tinygo build output");

        if !output_file.exists() {
            bail!(
                "TinyGo build completed but no artifact found at: {}",
                output_file.display()
            );
        }

        debug!(artifact_path = %output_file.display(), "found component artifact");
        Ok(output_file)
    }

    /// Build a TypeScript component using npm/node
    async fn build_typescript_component(&self, config: &Config) -> anyhow::Result<PathBuf> {
        debug!("building typescript component with npm");

        // Get TypeScript build configuration, use defaults if not specified
        let ts_config = config
            .build
            .as_ref()
            .and_then(|b| b.typescript.as_ref())
            .cloned()
            .unwrap_or_default();

        // Check if custom_command is specified - if so, use it instead of standard build
        if let Some(custom_command) = &ts_config.custom_command {
            return self.execute_custom_command(custom_command).await;
        }

        // Check if package.json has a build script
        let package_json_path = self.project_path.join("package.json");
        let package_json_content =
            std::fs::read_to_string(&package_json_path).context("failed to read package.json")?;

        let package_json: serde_json::Value =
            serde_json::from_str(&package_json_content).context("failed to parse package.json")?;

        // Check if there's a build script
        let build_command = &ts_config.build_command;
        if package_json
            .get("scripts")
            .and_then(|s| s.get(build_command))
            .is_some()
        {
            // Use explicit default from config
            let package_manager = &ts_config.package_manager;

            // Run install step before build to ensure dependencies are available
            let install_args = match package_manager.as_str() {
                "pnpm" => vec!["install".to_string()],
                "yarn" => vec!["install".to_string()],
                _ => vec!["install".to_string()], // default to npm
            };

            if !ts_config.skip_install {
                info!(package_manager = %package_manager, install_args = ?install_args, "running install command");

                let install_output = Command::new(package_manager)
                    .args(&install_args)
                    .current_dir(&self.project_path)
                    .output()
                    .context(format!("failed to execute {package_manager} install"))?;

                if !install_output.status.success() {
                    let stderr = String::from_utf8_lossy(&install_output.stderr);
                    error!(package_manager = %package_manager, stderr = %stderr, "install failed");
                    bail!("{package_manager} install failed: {stderr}");
                }

                let stdout = String::from_utf8_lossy(&install_output.stdout);
                debug!(package_manager = %package_manager, stdout = %stdout, "install output");
            } else {
                info!(package_manager = %package_manager, "skipping install step as per configuration");
            }

            // Build npm/pnpm/yarn command arguments
            let mut build_args = match package_manager.as_str() {
                "pnpm" => vec!["run".to_string(), build_command.to_string()],
                "yarn" => vec!["run".to_string(), build_command.to_string()],
                _ => vec!["run".to_string(), build_command.to_string()], // default to npm
            };

            // Add any additional build flags if configured
            for flag in &ts_config.build_flags {
                build_args.push(flag.clone());
            }

            debug!(package_manager = %package_manager, build_args = ?build_args, "running build command");

            // Run package manager build command
            let output = Command::new(package_manager)
                .args(&build_args)
                .current_dir(&self.project_path)
                .output()
                .context(format!("failed to execute {} run build", package_manager))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                error!(package_manager = %package_manager, stderr = %stderr, "build failed");
                bail!("{package_manager} build failed: {stderr}");
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            debug!(package_manager = %package_manager, stdout = %stdout, "build output");
        } else {
            warn!(
                build_command,
                "no build command found in package.json, skipping build step"
            );
        }

        // If an artifact path is specified, find the Wasm artifact there
        if let Some(artifact_path) = config.build.as_ref().and_then(|b| b.artifact_path.as_ref()) {
            if artifact_path.exists() {
                debug!(artifact_path = %artifact_path.display(), "found component artifact at specified path");
                Ok(artifact_path.to_owned())
            } else if self.project_path.join(artifact_path).exists() {
                debug!(
                    artifact_path = %artifact_path.display(),
                    "found component artifact at specified path relative to project root"
                );
                Ok(self.project_path.join(artifact_path))
            } else {
                bail!(
                    "specified artifact path does not exist: {}",
                    artifact_path.display()
                )
            }
        } else {
            // Look for common TypeScript build output locations
            let search_dirs = [
                self.project_path.join("dist"),
                self.project_path.join("build"),
                self.project_path.join("out"),
                self.project_path.clone(),
            ];

            // Search for any .wasm file in common output directories and project root
            for dir in &search_dirs {
                if dir.exists() && dir.is_dir() {
                    let entries = std::fs::read_dir(dir)
                        .context(format!("failed to read directory: {}", dir.display()))?;
                    for entry in entries {
                        let entry = entry.context("failed to read directory entry")?;
                        let path = entry.path();
                        if path.extension().and_then(|s| s.to_str()) == Some("wasm") {
                            debug!(artifact_path = %path.display(), "found component artifact");
                            return Ok(path);
                        }
                    }
                }
            }

            bail!(
                "No .wasm file found in common locations: dist/, build/, out/, or project root. Specify --artifact-path to override this behavior.",
            )
        }
    }

    /// Execute a custom build command, completely overriding standard build logic
    async fn execute_custom_command(&self, custom_command: &[String]) -> anyhow::Result<PathBuf> {
        let Some((command, args)) = custom_command.split_first() else {
            bail!("custom command must not be empty");
        };

        info!(command = command, args = ?args, "executing custom build command");

        let output = Command::new(command)
            .args(args)
            .current_dir(&self.project_path)
            .output()
            .context(format!("failed to execute custom command: {}", command))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(stderr = %stderr, "custom build command failed");
            bail!("custom build command '{command}' failed: {stderr}");
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        debug!(stdout = %stdout, "custom build command output");

        // TODO: we need to handle this better.
        // After running custom command, look for common build artifact locations
        let possible_paths = [
            self.project_path.join("build").join("component.wasm"),
            self.project_path.join("dist").join("component.wasm"),
            self.project_path.join("out").join("component.wasm"),
            self.project_path
                .join("target")
                .join("wasm32-wasip2")
                .join("release")
                .join("component.wasm"),
            self.project_path
                .join("target")
                .join("wasm32-wasip2")
                .join("debug")
                .join("component.wasm"),
            self.project_path
                .join("target")
                .join("wasm32-wasi")
                .join("release")
                .join("component.wasm"),
            self.project_path
                .join("target")
                .join("wasm32-wasi")
                .join("debug")
                .join("component.wasm"),
            self.project_path.join("component.wasm"),
        ];

        for path in &possible_paths {
            if path.exists() {
                debug!(
                    "found component artifact from custom command: {}",
                    path.display()
                );
                return Ok(path.clone());
            }
        }

        Err(anyhow::anyhow!(
            "custom build command completed successfully but no component.wasm artifact found in common locations"
        ))
    }

    /// Placeholder for pre-build hook
    async fn run_pre_build_hook(&self) -> anyhow::Result<()> {
        trace!("running pre-build hook (placeholder)");
        Ok(())
    }

    /// Placeholder for post-build hook  
    async fn run_post_build_hook(&self) -> anyhow::Result<()> {
        trace!("running post-build hook (placeholder)");
        Ok(())
    }
}
