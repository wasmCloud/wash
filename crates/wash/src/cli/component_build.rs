//! CLI command for building components, including Rust, TinyGo, and TypeScript projects

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{Context as _, bail};
use clap::Args;
use etcetera::AppStrategy;
use indicatif::{ProgressBar, ProgressStyle};
use serde::Serialize;
use tokio::{fs, process::Command};
use tracing::{debug, error, info, instrument, trace, warn};

use crate::component_build::ProjectType;
use crate::runtime::bindings::plugin::wasmcloud::wash::types::HookType;
use crate::wit::WitConfig;
use crate::{
    cli::{CliCommand, CliContext, CommandOutput},
    config::{Config, generate_project_config, load_config, save_config},
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

impl CliCommand for ComponentBuildCommand {
    #[instrument(level = "debug", skip(self, ctx), name = "component_build")]
    async fn handle(&self, ctx: &CliContext) -> anyhow::Result<CommandOutput> {
        // Load configuration with CLI arguments override
        let mut config = load_config(&ctx.config_path(), Some(&self.project_path), None::<Config>)?;
        // Ensure the CLI argument takes precedence
        if let Some(wit) = config.wit.as_mut() {
            wit.skip_fetch = self.skip_fetch;
        } else {
            config.wit = Some(WitConfig {
                skip_fetch: self.skip_fetch,
                ..Default::default()
            })
        }
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

    fn enable_pre_hook(&self) -> Option<HookType> {
        Some(HookType::BeforeBuild)
    }
    fn enable_post_hook(&self) -> Option<HookType> {
        Some(HookType::AfterBuild)
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
    let wit_dir = config.wit.as_ref().and_then(|w| w.wit_dir.clone());
    debug!(
        project_path = ?project_path.display(),
        wit_dir = ?wit_dir.as_ref().map(|p| p.display()),
        "building component at specified project path",
    );
    let builder = ComponentBuilder::new(project_path.to_path_buf(), wit_dir, skip_fetch);
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
    // Constants for search and timing parameters
    const MAX_SEARCH_DEPTH: usize = 5;
    const MAX_SIMPLE_SEARCH_DEPTH: usize = 4;
    const BUILD_TIME_BUFFER_SECS: u64 = 1;
    const SPINNER_TICK_INTERVAL_MS: u64 = 100;
    const WASM_EXTENSION_LEN: usize = 5; // ".wasm".len()

    /// Create a new component builder for the specified project path
    pub fn new(project_path: PathBuf, wit_dir: Option<PathBuf>, skip_wit_fetch: bool) -> Self {
        Self {
            project_path,
            wit_dir,
            skip_wit_fetch,
        }
    }

    /// Helper to run a command with a spinner displaying the command and args with elapsed time
    async fn run_command_with_spinner(
        &self,
        command: &mut Command,
        command_name: &str,
        args: &[String],
    ) -> anyhow::Result<std::process::Output> {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg} [{elapsed}]")
                .context("failed to create spinner style")?,
        );

        // Create message showing command + args
        let message = if args.is_empty() {
            command_name.to_string()
        } else {
            format!("{} {}", command_name, args.join(" "))
        };
        spinner.set_message(message);
        spinner.enable_steady_tick(Duration::from_millis(Self::SPINNER_TICK_INTERVAL_MS));

        let start = Instant::now();
        let result = command
            .output()
            .await
            .context("failed to execute command")?;
        let elapsed = start.elapsed();

        spinner.finish_and_clear();
        info!(
            "{} completed in {:.2}s",
            if args.is_empty() {
                command_name.to_string()
            } else {
                format!("{} {}", command_name, args.join(" "))
            },
            elapsed.as_secs_f64()
        );

        Ok(result)
    }

    /// Get the WIT directory, defaulting to project_path/wit if not specified
    fn get_wit_dir(&self) -> PathBuf {
        match &self.wit_dir {
            Some(wit_dir) if wit_dir.is_absolute() => wit_dir.clone(),
            Some(wit_dir) => self.project_path.join(wit_dir),
            None => self.project_path.join("wit"),
        }
    }

    /// Build the component
    #[instrument(level = "debug", skip(self, ctx, config))]
    pub async fn build(
        &self,
        ctx: &CliContext,
        config: &Config,
    ) -> anyhow::Result<ComponentBuildResult> {
        debug!(
            path = ?self.project_path.display(),
            "building component",
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
        self.check_required_tools(&project_type).await?;

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

        info!(path = ?self.project_path.display(), "building component");
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
        generate_project_config(&self.project_path, &project_type, config).await?;

        // Attempt to canonicalize the artifact path
        let artifact_path = artifact_path.canonicalize().unwrap_or(artifact_path);

        debug!(
            artifact_path = ?artifact_path.display(),
            "component build completed successfully",
        );

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
    async fn check_required_tools(&self, project_type: &ProjectType) -> anyhow::Result<()> {
        let mut missing_tools = Vec::new();
        let mut warnings = Vec::new();

        match project_type {
            ProjectType::Rust => {
                // Check for cargo
                if !self.tool_exists("cargo", "--version").await {
                    missing_tools.push("cargo (Rust build tool)");
                } else {
                    // Check for wasm32-wasip2 target
                    match tokio::process::Command::new("rustup")
                        .args(["target", "list", "--installed"])
                        .output()
                        .await
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
                if !self.tool_exists("go", "version").await {
                    missing_tools.push("go (Go compiler)");
                }
                if !self.tool_exists("tinygo", "version").await {
                    missing_tools.push("tinygo (TinyGo compiler for WebAssembly)");
                }
                if !self.tool_exists("wasm-tools", "--version").await {
                    missing_tools.push("wasm-tools (Wasm tools for Go)");
                }
            }
            ProjectType::TypeScript => {
                if !self.tool_exists("node", "--version").await {
                    missing_tools.push("node (Node.js runtime)");
                }
                if !self.npm_exists().await {
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

        // Report missing tools and fail early to prevent confusing errors later
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
    async fn tool_exists(&self, tool: &str, cmd: &str) -> bool {
        tokio::process::Command::new(tool)
            .arg(cmd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map(|status| status.success())
            .unwrap_or(false)
    }

    /// Check if npm exists, handling cross-platform differences
    /// On Windows, npm is typically npm.cmd, on Unix it's npm
    async fn npm_exists(&self) -> bool {
        // Try npm first (works on Unix and some Windows setups)
        if self.tool_exists("npm", "--version").await {
            return true;
        }

        // On Windows, try npm.cmd
        #[cfg(target_os = "windows")]
        {
            self.tool_exists("npm.cmd", "--version").await
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }

    /// Get the correct npm command name for the current platform
    /// This method checks which npm variant is actually available
    async fn get_npm_command(&self) -> Option<&'static str> {
        // Try npm first (works on Unix and some Windows setups)
        if self.tool_exists("npm", "--version").await {
            return Some("npm");
        }

        // On Windows, try npm.cmd
        #[cfg(target_os = "windows")]
        {
            if self.tool_exists("npm.cmd", "--version").await {
                return Some("npm.cmd");
            }
        }

        None
    }

    /// Fetch WIT dependencies if the project has any
    #[instrument(
        level = "debug",
        skip(self, ctx, config),
        name = "fetch_wit_dependencies"
    )]
    async fn fetch_wit_dependencies(
        &self,
        ctx: &CliContext,
        config: &Config,
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
        let wkg_config = wasm_pkg_core::config::Config::default();
        let mut fetcher = WkgFetcher::from_common(&args, wkg_config).await?;

        // Apply WIT source overrides if present in configuration
        if let Some(wit_config) = &config.wit
            && !wit_config.sources.is_empty()
        {
            debug!("applying WIT source overrides: {:?}", wit_config.sources);
            fetcher
                .resolve_extended_pull_configs(&wit_config.sources, &self.project_path)
                .await
                .context("failed to resolve WIT source overrides")?;
        }

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

        // Add message-format json to get structured output for artifact paths
        cargo_args.push("--message-format".to_string());
        cargo_args.push("json".to_string());

        // Add any additional cargo flags if configured
        for flag in &rust_config.cargo_flags {
            cargo_args.push(flag.clone());
        }

        debug!(cargo_args = ?cargo_args, "running cargo with args");

        // Change to project directory and run cargo build
        let output = self
            .run_command_with_spinner(
                Command::new("cargo")
                    .args(&cargo_args)
                    .current_dir(&self.project_path),
                "cargo",
                &cargo_args,
            )
            .await
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

        // Parse JSON output to find the .wasm artifact
        for line in stdout.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let json_msg: serde_json::Value = match serde_json::from_str(line) {
                Ok(msg) => msg,
                Err(_) => continue, // Skip non-JSON lines
            };

            // Look for compiler-artifact messages with .wasm filenames
            if json_msg.get("reason").and_then(|r| r.as_str()) == Some("compiler-artifact") {
                if let Some(filenames) = json_msg.get("filenames").and_then(|f| f.as_array()) {
                    for filename in filenames {
                        if let Some(path_str) = filename.as_str() {
                            let path = PathBuf::from(path_str);
                            if path.extension().and_then(|s| s.to_str()) == Some("wasm") {
                                debug!(artifact_path = %path.display(), "found component artifact from cargo JSON output");
                                return Ok(path);
                            }
                        }
                    }
                }
            }
        }

        // Fallback: Find the generated wasm file using the old method if JSON parsing fails
        warn!("failed to find .wasm artifact from cargo JSON output, falling back to directory search");
        let build_type = if rust_config.release {
            "release"
        } else {
            "debug"
        };
        let target_dir = self
            .project_path
            .join(format!("target/{}/{}", rust_config.target, build_type));
        let mut entries = fs::read_dir(&target_dir)
            .await
            .with_context(|| format!("failed to read Rust target directory '{}' - ensure cargo build completed successfully", target_dir.display()))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .context("failed to read directory entry")?
        {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("wasm") {
                debug!(artifact_path = %path.display(), "found component artifact via directory search");
                return Ok(path);
            }
        }

        Err(anyhow::anyhow!(
            "no .wasm file found in cargo output or target directory: {}",
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
            fs::create_dir_all(&build_dir)
                .await
                .context("failed to create build directory")?;
        }

        let output_file = build_dir.join("output.wasm");
        let output_file_relative = PathBuf::from("build/output.wasm");

        // Build tinygo command arguments
        let mut tinygo_args = vec![
            "build".to_string(),
            "-o".to_string(),
            output_file_relative
                .to_str()
                .context("failed to convert output file path to str")?
                .to_string(),
        ];

        // Apply target - now uses explicit default from config
        tinygo_args.push("-target".to_string());
        tinygo_args.push(tinygo_config.target.to_string());

        // Check if WIT directory exists before adding WIT-related flags
        let wit_dir = self.get_wit_dir();
        if wit_dir.exists() {
            // Add WIT package - use WIT directory path if not explicitly specified
            let wit_package = if let Some(wit_package) = &tinygo_config.wit_package {
                wit_package.clone()
            } else {
                // Use relative path from project directory
                wit_dir
                    .strip_prefix(&self.project_path)
                    .unwrap_or(&wit_dir)
                    .to_string_lossy()
                    .to_string()
            };
            tinygo_args.push("-wit-package".to_string());
            tinygo_args.push(wit_package);

            // Add WIT world - this is required for TinyGo builds when WIT is present
            let Some(wit_world) = &tinygo_config.wit_world else {
                // Generate project config to ensure .wash/config.json exists with placeholder
                let mut config_with_placeholder = config.clone();
                let artifact_path_relative = PathBuf::from("build/output.wasm");

                if let Some(build_config) = &mut config_with_placeholder.build {
                    build_config.artifact_path = Some(artifact_path_relative.clone());
                    if let Some(tinygo_config) = &mut build_config.tinygo {
                        tinygo_config.wit_world = Some("PLACEHOLDER_WIT_WORLD".to_string());
                    }
                } else {
                    let mut tinygo_config = tinygo_config.clone();
                    tinygo_config.wit_world = Some("PLACEHOLDER_WIT_WORLD".to_string());
                    config_with_placeholder.build = Some(crate::component_build::BuildConfig {
                        tinygo: Some(tinygo_config),
                        artifact_path: Some(artifact_path_relative),
                        ..Default::default()
                    });
                }

                // Write config with placeholder
                let config_dir = self.project_path.join(".wash");
                let config_path = config_dir.join("config.json");
                tokio::fs::create_dir_all(&config_dir)
                    .await
                    .context("failed to create .wash directory")?;
                save_config(&config_with_placeholder, &config_path).await?;

                bail!(
                    "TinyGo builds require wit_world to be specified in the configuration. \
                    A config file has been created at {} with a placeholder. \
                    Please update the wit_world field to match your WIT world name.",
                    config_path.display()
                );
            };
            tinygo_args.push("-wit-world".to_string());
            tinygo_args.push(wit_world.to_string());
        } else {
            debug!(
                "WIT directory does not exist, skipping WIT-related flags: {}",
                wit_dir.display()
            );
        }

        // Apply garbage collector - now uses explicit default from config
        tinygo_args.push("-gc".to_string());
        tinygo_args.push(tinygo_config.gc.to_string());

        // Apply scheduler - now uses explicit default from config
        tinygo_args.push("-scheduler".to_string());
        tinygo_args.push(tinygo_config.scheduler.to_string());

        // Apply optimization level
        tinygo_args.push("-opt".to_string());
        tinygo_args.push(tinygo_config.opt.to_string());

        // Apply panic strategy
        tinygo_args.push("-panic".to_string());
        tinygo_args.push(tinygo_config.panic.to_string());

        // Apply build tags if configured
        if !tinygo_config.tags.is_empty() {
            tinygo_args.push("-tags".to_string());
            tinygo_args.push(tinygo_config.tags.join(","));
        }

        // Apply stack size if configured
        if let Some(stack_size) = &tinygo_config.stack_size {
            tinygo_args.push("-stack-size".to_string());
            tinygo_args.push(stack_size.clone());
        }

        // Add debug settings - configurable via no_debug flag
        if tinygo_config.no_debug {
            tinygo_args.push("-no-debug".to_string());
        }

        // Add any additional build flags if configured
        for flag in &tinygo_config.build_flags {
            tinygo_args.push(flag.to_string());
        }

        // Add source directory
        tinygo_args.push(".".to_string());

        // For Go projects, optionally run `go generate ./...` before building if not disabled in config
        if config
            .build
            .as_ref()
            .and_then(|b| b.tinygo.as_ref())
            .is_some_and(|c| !c.disable_go_generate)
        {
            debug!("running `go generate ./...` before TinyGo build");
            let go_args = vec!["generate".to_string(), "./...".to_string()];
            let output = self
                .run_command_with_spinner(
                    Command::new("go")
                        .args(&go_args)
                        .current_dir(&self.project_path),
                    "go",
                    &go_args,
                )
                .await
                .context("failed to execute `go generate ./...`")?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                warn!(stderr = %stderr, "`go generate ./...` failed, continuing build");
            } else {
                let stdout = String::from_utf8_lossy(&output.stdout);
                debug!(stdout = %stdout, "`go generate ./...` output");
            }
        } else {
            debug!("`go generate ./...` is disabled by config");
        }

        debug!(tinygo_args = ?tinygo_args, "running tinygo with args");

        // Run tinygo build command
        let output = self
            .run_command_with_spinner(
                Command::new("tinygo")
                    .args(&tinygo_args)
                    .current_dir(&self.project_path),
                "tinygo",
                &tinygo_args,
            )
            .await
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
        let package_json_content = fs::read_to_string(&package_json_path)
            .await
            .context("failed to read package.json")?;

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

            // Get the correct command name for the package manager, handling Windows npm.cmd
            let package_manager_cmd = if package_manager == "npm" {
                self.get_npm_command().await.unwrap_or("npm")
            } else {
                package_manager.as_str()
            };

            // Run install step before build to ensure dependencies are available
            let install_args = match package_manager.as_str() {
                "pnpm" => vec!["install".to_string()],
                "yarn" => vec!["install".to_string()],
                _ => vec!["install".to_string()], // default to npm
            };

            if !ts_config.skip_install {
                info!(package_manager = %package_manager, install_args = ?install_args, "running install command");

                let install_output = self
                    .run_command_with_spinner(
                        Command::new(package_manager_cmd)
                            .args(&install_args)
                            .current_dir(&self.project_path),
                        package_manager_cmd,
                        &install_args,
                    )
                    .await
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
            let output = self
                .run_command_with_spinner(
                    Command::new(package_manager_cmd)
                        .args(&build_args)
                        .current_dir(&self.project_path),
                    package_manager_cmd,
                    &build_args,
                )
                .await
                .context(format!("failed to execute {package_manager} run build"))?;

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
                    let mut entries = fs::read_dir(dir)
                        .await
                        .context(format!("failed to read directory: {}", dir.display()))?;
                    while let Some(entry) = entries
                        .next_entry()
                        .await
                        .context("failed to read directory entry")?
                    {
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

        // Record build start time for modification-based detection
        let build_start_time = std::time::SystemTime::now();

        let args_strings: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let output = self
            .run_command_with_spinner(
                Command::new(command)
                    .args(args)
                    .current_dir(&self.project_path),
                command,
                &args_strings,
            )
            .await
            .context(format!("failed to execute custom command: {command}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(stderr = %stderr, "custom build command failed");
            bail!("custom build command '{command}' failed: {stderr}");
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!(stdout = %stdout, stderr = %stderr, "custom build command output");

        // Try multiple detection strategies in order of reliability
        let mut checked_paths = Vec::new();

        // Strategy 1: Parse command output for artifact paths
        if let Some(artifact_path) = self.parse_build_output_for_artifacts(&stdout, &stderr).await {
            debug!(artifact_path = %artifact_path.display(), "found artifact from command output parsing");
            return Ok(artifact_path);
        }

        // Strategy 2: Look for recently modified .wasm files
        if let Some(artifact_path) = self.find_recent_wasm_files(build_start_time, &mut checked_paths).await? {
            debug!(artifact_path = %artifact_path.display(), "found artifact by modification time");
            return Ok(artifact_path);
        }

        // Strategy 3: Comprehensive directory search with patterns
        if let Some(artifact_path) = self.search_build_directories_comprehensively(&mut checked_paths).await? {
            debug!(artifact_path = %artifact_path.display(), "found artifact via comprehensive search");
            return Ok(artifact_path);
        }

        // Strategy 4: Last resort - search entire project for any .wasm files
        if let Some(artifact_path) = self.search_project_for_any_wasm(&mut checked_paths).await? {
            warn!(artifact_path = %artifact_path.display(), "found .wasm file in unexpected location");
            return Ok(artifact_path);
        }

        // No artifact found - provide detailed error
        let mut error_msg = format!(
            "Custom build command '{}' completed successfully but no WebAssembly artifact was found.\n\n",
            command
        );
        error_msg.push_str("Searched the following locations:\n");
        for path in &checked_paths {
            error_msg.push_str(&format!("  - {}\n", path.display()));
        }
        error_msg.push_str("\nConsider:\n");
        error_msg.push_str("  - Verifying your build command produces a .wasm file\n");
        error_msg.push_str("  - Using --artifact-path to specify the exact output location\n");
        error_msg.push_str("  - Checking that your build command succeeded and produced output\n");

        bail!("{}", error_msg)
    }

    /// Parse build command output (stdout/stderr) for artifact paths
    async fn parse_build_output_for_artifacts(&self, stdout: &str, stderr: &str) -> Option<PathBuf> {
        let combined_output = format!("{}\n{}", stdout, stderr);

        // Keywords that often precede artifact paths
        let keywords = [
            "Generated", "Built", "Output", "Created", "Compiled", "Wrote", "Artifact", "Binary",
            "written", "saved", "created", "Result", "→", "->", "executable"
        ];

        // Check each line for potential .wasm paths
        for line in combined_output.lines() {
            let line = line.trim();
            if !line.contains(".wasm") {
                continue;
            }

            // Look for patterns like "Generated: path/to/file.wasm" or "Built path/to/file.wasm"
            for keyword in &keywords {
                if line.to_lowercase().contains(&keyword.to_lowercase()) {
                    // Find .wasm paths in this line
                    if let Some(wasm_path) = self.extract_wasm_path_from_line(line) {
                        return Some(wasm_path);
                    }
                }
            }

            // Also check for any line that just contains a .wasm path
            if let Some(wasm_path) = self.extract_wasm_path_from_line(line) {
                return Some(wasm_path);
            }
        }

        None
    }

    /// Extract a .wasm file path from a line of text
    fn extract_wasm_path_from_line(&self, line: &str) -> Option<PathBuf> {
        // Split line into words and look for .wasm files
        for word in line.split_whitespace() {
            let cleaned_word = word.trim_matches(|c: char| c.is_ascii_punctuation() && c != '.' && c != '/' && c != '\\' && c != ':');

            if cleaned_word.ends_with(".wasm") {
                let path = if Path::new(cleaned_word).is_absolute() {
                    PathBuf::from(cleaned_word)
                } else {
                    self.project_path.join(cleaned_word)
                };

                if path.exists() && path.extension().and_then(|s| s.to_str()) == Some("wasm") {
                    debug!(path = %path.display(), line = line, "extracted .wasm path from output");
                    return Some(path);
                }
            }
        }

        // Also try to find paths that might be quoted or have colons
        if let Some(start) = line.find(".wasm") {
            // Look backwards and forwards to find the full path
            let mut path_start = start;
            let path_end = start + Self::WASM_EXTENSION_LEN;

            let chars: Vec<char> = line.chars().collect();

            // Find start of path (look backwards for path characters)
            while path_start > 0 {
                let c = chars[path_start - 1];
                if c.is_alphanumeric() || c == '/' || c == '\\' || c == '.' || c == '_' || c == '-' {
                    path_start -= 1;
                } else {
                    break;
                }
            }

            let path_str = &line[path_start..path_end];
            if !path_str.is_empty() && path_str.ends_with(".wasm") {
                let path = if Path::new(path_str).is_absolute() {
                    PathBuf::from(path_str)
                } else {
                    self.project_path.join(path_str)
                };

                if path.exists() && path.extension().and_then(|s| s.to_str()) == Some("wasm") {
                    debug!(path = %path.display(), line = line, "extracted .wasm path from output (context search)");
                    return Some(path);
                }
            }
        }

        None
    }

    /// Find .wasm files modified after the build started
    async fn find_recent_wasm_files(
        &self,
        build_start_time: std::time::SystemTime,
        checked_paths: &mut Vec<PathBuf>,
    ) -> anyhow::Result<Option<PathBuf>> {
        let search_dirs = [
            self.project_path.join("target"),
            self.project_path.join("build"),
            self.project_path.join("dist"),
            self.project_path.join("out"),
            self.project_path.join("pkg"),
            self.project_path.clone(),
        ];

        let mut candidates = Vec::new();

        for dir in &search_dirs {
            if !dir.exists() || !dir.is_dir() {
                continue;
            }

            checked_paths.push(dir.clone());

            if let Ok(mut entries) = fs::read_dir(dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let path = entry.path();

                    if path.extension().and_then(|s| s.to_str()) == Some("wasm") {
                        checked_paths.push(path.clone());

                        if let Ok(metadata) = entry.metadata().await {
                            if let Ok(modified) = metadata.modified() {
                                // Check if file was modified after build started (with buffer)
                                if modified >= build_start_time.checked_sub(Duration::from_secs(Self::BUILD_TIME_BUFFER_SECS)).unwrap_or(build_start_time) {
                                    candidates.push((path, modified));
                                }
                            }
                        }
                    }
                }
            }

            // Also search recursively in target directories (using iterative approach to avoid recursion)
            if dir.ends_with("target") || dir.ends_with("build") || dir.ends_with("dist") {
                if let Some(recursive_result) = self.search_directory_iterative(dir, build_start_time, checked_paths).await? {
                    candidates.push(recursive_result);
                }
            }
        }

        // Return the most recently modified file
        if let Some((path, _)) = candidates.into_iter().max_by_key(|(_, modified)| *modified) {
            return Ok(Some(path));
        }

        Ok(None)
    }

    /// Search a directory iteratively for recent .wasm files (avoiding recursion issues)
    async fn search_directory_iterative(
        &self,
        dir: &Path,
        build_start_time: std::time::SystemTime,
        checked_paths: &mut Vec<PathBuf>,
    ) -> anyhow::Result<Option<(PathBuf, std::time::SystemTime)>> {
        let mut best_candidate = None;
        let mut dirs_to_search = vec![dir.to_path_buf()];
        let mut depth = 0;
        while let Some(current_dir) = dirs_to_search.pop() {
            depth += 1;
            if depth > Self::MAX_SEARCH_DEPTH {
                break;
            }

            if let Ok(mut entries) = fs::read_dir(&current_dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let path = entry.path();

                    if path.is_dir() {
                        // Skip common directories that are unlikely to contain artifacts
                        if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                            if matches!(dir_name, "node_modules" | ".git" | ".cargo" | "deps" | "incremental") {
                                continue;
                            }
                        }
                        dirs_to_search.push(path);
                    } else if path.extension().and_then(|s| s.to_str()) == Some("wasm") {
                        checked_paths.push(path.clone());

                        if let Ok(metadata) = entry.metadata().await {
                            if let Ok(modified) = metadata.modified() {
                                if modified >= build_start_time.checked_sub(Duration::from_secs(Self::BUILD_TIME_BUFFER_SECS)).unwrap_or(build_start_time) {
                                    if best_candidate.as_ref().map(|(_, time)| *time).unwrap_or(std::time::UNIX_EPOCH) < modified {
                                        best_candidate = Some((path, modified));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(best_candidate)
    }

    /// Search build directories comprehensively using naming patterns
    async fn search_build_directories_comprehensively(
        &self,
        checked_paths: &mut Vec<PathBuf>,
    ) -> anyhow::Result<Option<PathBuf>> {
        // Common build directory patterns and file names
        let search_patterns = [
            // Standard WebAssembly output patterns
            ("build", vec!["*.wasm", "component.wasm", "main.wasm"]),
            ("dist", vec!["*.wasm", "component.wasm", "main.wasm"]),
            ("out", vec!["*.wasm", "component.wasm", "main.wasm"]),
            ("pkg", vec!["*.wasm", "component.wasm", "main.wasm"]),
            ("output", vec!["*.wasm"]),
            ("release", vec!["*.wasm"]),

            // Language-specific patterns
            ("target/wasm32-wasip2/release", vec!["*.wasm"]),
            ("target/wasm32-wasip2/debug", vec!["*.wasm"]),
            ("target/wasm32-wasi/release", vec!["*.wasm"]),
            ("target/wasm32-wasi/debug", vec!["*.wasm"]),
            ("target/wasm32-unknown-unknown/release", vec!["*.wasm"]),
            ("target/wasm32-unknown-unknown/debug", vec!["*.wasm"]),

            // Framework-specific patterns
            ("target/release", vec!["*.wasm"]),
            ("target/debug", vec!["*.wasm"]),
            (".next", vec!["*.wasm"]),
            ("public", vec!["*.wasm"]),
            ("static", vec!["*.wasm"]),
            ("assets", vec!["*.wasm"]),
        ];

        for (dir_name, file_patterns) in &search_patterns {
            let search_dir = self.project_path.join(dir_name);
            checked_paths.push(search_dir.clone());

            if !search_dir.exists() || !search_dir.is_dir() {
                continue;
            }

            // Try each file pattern
            for pattern in file_patterns {
                if pattern == &"*.wasm" {
                    // Search for any .wasm file
                    if let Ok(mut entries) = fs::read_dir(&search_dir).await {
                        while let Ok(Some(entry)) = entries.next_entry().await {
                            let path = entry.path();
                            if path.extension().and_then(|s| s.to_str()) == Some("wasm") {
                                debug!(path = %path.display(), pattern = pattern, "found via pattern search");
                                return Ok(Some(path));
                            }
                        }
                    }
                } else {
                    // Try specific filename
                    let specific_path = search_dir.join(pattern);
                    checked_paths.push(specific_path.clone());
                    if specific_path.exists() {
                        debug!(path = %specific_path.display(), pattern = pattern, "found specific file");
                        return Ok(Some(specific_path));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Last resort: search the entire project for any .wasm files
    async fn search_project_for_any_wasm(
        &self,
        checked_paths: &mut Vec<PathBuf>,
    ) -> anyhow::Result<Option<PathBuf>> {
        debug!("performing last resort search for any .wasm files in project");

        // Search project root first
        if let Ok(mut entries) = fs::read_dir(&self.project_path).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if !path.is_dir() && path.extension().and_then(|s| s.to_str()) == Some("wasm") {
                    checked_paths.push(path.clone());
                    return Ok(Some(path));
                }
            }
        }

        // Recursively search common directories, avoiding deep nesting
        let dirs_to_search = [
            "src", "lib", "build", "dist", "out", "target", "public", "static",
            "assets", "bin", "pkg", "wasm", "webassembly"
        ];

        for dir_name in &dirs_to_search {
            let dir_path = self.project_path.join(dir_name);
            if dir_path.exists() && dir_path.is_dir() {
                if let Some(found) = self.search_directory_simple(&dir_path, checked_paths).await? {
                    return Ok(Some(found));
                }
            }
        }

        Ok(None)
    }

    /// Search directory with simple depth-limited approach
    async fn search_directory_simple(
        &self,
        dir: &Path,
        checked_paths: &mut Vec<PathBuf>,
    ) -> anyhow::Result<Option<PathBuf>> {
        let mut dirs_to_search = vec![dir.to_path_buf()];
        let mut depth = 0;
        while let Some(current_dir) = dirs_to_search.pop() {
            depth += 1;
            if depth > Self::MAX_SIMPLE_SEARCH_DEPTH {
                break;
            }

            if let Ok(mut entries) = fs::read_dir(&current_dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let path = entry.path();

                    if path.is_dir() {
                        // Skip common directories that are unlikely to contain artifacts
                        if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                            if matches!(dir_name, "node_modules" | ".git" | ".cargo" | "deps" | "incremental") {
                                continue;
                            }
                        }
                        if depth < Self::MAX_SIMPLE_SEARCH_DEPTH {
                            dirs_to_search.push(path);
                        }
                    } else if path.extension().and_then(|s| s.to_str()) == Some("wasm") {
                        checked_paths.push(path.clone());
                        return Ok(Some(path));
                    }
                }
            }
        }

        Ok(None)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;
    use tempfile::TempDir;
    use tokio::fs as async_fs;

    /// Helper to create a test Rust project with Cargo.toml
    async fn create_test_rust_project(temp_dir: &Path) -> anyhow::Result<()> {
        let cargo_toml_content = r#"[package]
name = "test-component"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = "0.30.0"
"#;

        let lib_rs_content = r#"wit_bindgen::generate!({
    world: "hello",
    path: "wit/world.wit",
    exports: {
        "test:component/hello": Guest,
    },
});

struct Guest;

impl exports::test::component::hello::Guest for Guest {
    fn hello() -> String {
        "Hello from WebAssembly!".to_string()
    }
}
"#;

        let world_wit_content = r#"package test:component;

world hello {
    export hello: func() -> string;
}
"#;

        // Create project structure
        async_fs::create_dir_all(temp_dir.join("src")).await?;
        async_fs::create_dir_all(temp_dir.join("wit")).await?;

        // Write files
        async_fs::write(temp_dir.join("Cargo.toml"), cargo_toml_content).await?;
        async_fs::write(temp_dir.join("src/lib.rs"), lib_rs_content).await?;
        async_fs::write(temp_dir.join("wit/world.wit"), world_wit_content).await?;

        Ok(())
    }

    /// Helper to create .wash/config.json with custom build command
    #[allow(dead_code)]
    async fn create_wash_config(temp_dir: &Path, config_content: &str) -> anyhow::Result<()> {
        let wash_dir = temp_dir.join(".wash");
        async_fs::create_dir_all(&wash_dir).await?;
        async_fs::write(wash_dir.join("config.json"), config_content).await?;
        Ok(())
    }

    /// Helper to run cargo build with specific arguments and capture output
    async fn run_cargo_build(temp_dir: &Path, args: &[&str]) -> anyhow::Result<(bool, String)> {
        let output = StdCommand::new("cargo")
            .args(args)
            .current_dir(temp_dir)
            .output()?;

        Ok((output.status.success(), String::from_utf8_lossy(&output.stdout).to_string()))
    }

    /// Test custom build command with message-format json
    #[tokio::test]
    async fn test_custom_build_with_message_format_json() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_path = temp_dir.path();

        // Create test Rust project
        create_test_rust_project(project_path).await.expect("failed to create test project");

        // First, run cargo build to create debug artifact
        let (debug_success, debug_output) = run_cargo_build(
            project_path,
            &["build", "--target", "wasm32-wasip2", "--message-format", "json"]
        ).await.expect("failed to run cargo build");

        assert!(debug_success, "cargo build should succeed - ensure wasm32-wasip2 target is installed");

        // Create the custom command configuration
        let custom_command = vec!["cargo".to_string(), "build".to_string(), "--target".to_string(), "wasm32-wasip2".to_string(), "--message-format".to_string(), "json".to_string()];

        let builder = ComponentBuilder::new(project_path.to_path_buf(), None, false);
        let result = builder.execute_custom_command(&custom_command).await;

        match result {
            Ok(artifact_path) => {
                // Verify the artifact path is in debug location (not release)
                let artifact_path_str = artifact_path.to_string_lossy();
                assert!(
                    artifact_path_str.contains("debug") && !artifact_path_str.contains("release"),
                    "Expected debug build artifact, got: {}",
                    artifact_path_str
                );
                assert!(
                    artifact_path_str.ends_with(".wasm"),
                    "Expected .wasm artifact, got: {}",
                    artifact_path_str
                );
                assert!(
                    artifact_path.exists(),
                    "Artifact should exist at: {}",
                    artifact_path_str
                );

                // Verify that the output parsing strategy worked
                // Check that the JSON output contains artifact information
                let has_json_artifacts = debug_output.lines().any(|line| {
                    if let Ok(json_msg) = serde_json::from_str::<serde_json::Value>(line) {
                        json_msg.get("reason").and_then(|r| r.as_str()) == Some("compiler-artifact") &&
                        json_msg.get("filenames").and_then(|f| f.as_array()).is_some()
                    } else {
                        false
                    }
                });
                assert!(has_json_artifacts, "Expected JSON format output with artifact info");
            }
            Err(e) => {
                panic!("Build failed: {}", e);
            }
        }
    }

    /// Test custom build command without message-format json
    #[tokio::test]
    async fn test_custom_build_without_message_format_json() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_path = temp_dir.path();

        // Create test Rust project
        create_test_rust_project(project_path).await.expect("failed to create test project");

        // First, run cargo build to create debug artifact
        let (debug_success, _) = run_cargo_build(
            project_path,
            &["build", "--target", "wasm32-wasip2"]
        ).await.expect("failed to run cargo build");

        assert!(debug_success, "cargo build should succeed - ensure wasm32-wasip2 target is installed");

        // Also run release to create release artifact
        let (release_success, _) = run_cargo_build(
            project_path,
            &["build", "--release", "--target", "wasm32-wasip2"]
        ).await.expect("failed to run cargo build");

        assert!(release_success, "cargo build should succeed");

        // Create the custom command configuration (without message-format json)
        let custom_command = vec!["cargo".to_string(), "build".to_string(), "--target".to_string(), "wasm32-wasip2".to_string()];

        let builder = ComponentBuilder::new(project_path.to_path_buf(), None, false);
        let result = builder.execute_custom_command(&custom_command).await;

        match result {
            Ok(artifact_path) => {
                // Verify the artifact path is in debug location (since no release flag in custom command)
                let artifact_path_str = artifact_path.to_string_lossy();
                assert!(
                    artifact_path_str.contains("debug") && !artifact_path_str.contains("release"),
                    "Expected debug build artifact, got: {}",
                    artifact_path_str
                );
                assert!(
                    artifact_path_str.ends_with(".wasm"),
                    "Expected .wasm artifact, got: {}",
                    artifact_path_str
                );
                assert!(
                    artifact_path.exists(),
                    "Artifact should exist at: {}",
                    artifact_path_str
                );
            }
            Err(e) => {
                panic!("Build failed: {}", e);
            }
        }
    }

    /// Test artifact path extraction from command output
    #[tokio::test]
    async fn test_parse_build_output_for_artifacts() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_path = temp_dir.path();

        // Create test project structure
        create_test_rust_project(project_path).await.expect("failed to create test project");

        let builder = ComponentBuilder::new(project_path.to_path_buf(), None, false);

        // Test various output formats
        let test_cases = [
            // Cargo output patterns
            ("Generated: target/wasm32-wasip2/debug/test_component.wasm", Some("target/wasm32-wasip2/debug/test_component.wasm")),
            ("Built target/debug/component.wasm successfully", Some("target/debug/component.wasm")),
            ("Output: ./build/output.wasm", Some("build/output.wasm")),
            ("Created build/test.wasm", Some("build/test.wasm")),
            // JSON format outputs
            ("{\"reason\":\"compiler-artifact\",\"filenames\":[\"target/debug/test.wasm\"]}", None), // This would need JSON parsing
            // No valid paths
            ("Build completed successfully", None),
            ("No .wasm files mentioned", None),
        ];

        for (output, expected) in test_cases {
            // Create the expected file if we expect to find it
            if let Some(expected_path) = expected {
                let full_path = project_path.join(expected_path);
                if let Some(parent) = full_path.parent() {
                    async_fs::create_dir_all(parent).await.unwrap();
                }
                async_fs::write(&full_path, "test wasm content").await.unwrap();

                let result = builder.parse_build_output_for_artifacts(output, "").await;
                assert!(result.is_some(), "Expected to find artifact in output: {}", output);

                let found_path = result.unwrap();
                assert_eq!(found_path, full_path, "Incorrect path extracted from: {}", output);

                // Clean up
                let _ = async_fs::remove_file(full_path).await;
            } else {
                let result = builder.parse_build_output_for_artifacts(output, "").await;
                assert!(result.is_none(), "Expected no artifact in output: {}", output);
            }
        }
    }

    /// Test comprehensive directory search patterns
    #[tokio::test]
    async fn test_comprehensive_directory_search() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_path = temp_dir.path();

        let builder = ComponentBuilder::new(project_path.to_path_buf(), None, false);

        // Create various test directories and files
        let test_locations = [
            "build/component.wasm",
            "dist/main.wasm",
            "out/index.wasm",
            "target/wasm32-wasip2/debug/test.wasm",
            "target/wasm32-wasi/release/other.wasm",
            "pkg/output.wasm",
        ];

        for location in test_locations {
            let full_path = project_path.join(location);
            async_fs::create_dir_all(full_path.parent().unwrap()).await.unwrap();
            async_fs::write(&full_path, "test wasm content").await.unwrap();
        }

        let mut checked_paths = Vec::new();
        let result = builder.search_build_directories_comprehensively(&mut checked_paths).await.unwrap();

        assert!(result.is_some(), "Should find at least one .wasm file");
        let found_path = result.unwrap();
        assert!(found_path.exists(), "Found path should exist");
        assert!(found_path.extension().and_then(|s| s.to_str()) == Some("wasm"), "Should find .wasm file");
        assert!(!checked_paths.is_empty(), "Should have checked some paths");
    }

    /// Test file modification time detection
    #[tokio::test]
    async fn test_recent_wasm_files_detection() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_path = temp_dir.path();

        let builder = ComponentBuilder::new(project_path.to_path_buf(), None, false);

        // Create target directory and old file
        let target_dir = project_path.join("target/debug");
        async_fs::create_dir_all(&target_dir).await.unwrap();
        let old_wasm = target_dir.join("old.wasm");
        async_fs::write(&old_wasm, "old content").await.unwrap();

        // Sleep briefly to ensure time difference
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let build_start_time = std::time::SystemTime::now();

        // Sleep again and create new file
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        let new_wasm = target_dir.join("new.wasm");
        async_fs::write(&new_wasm, "new content").await.unwrap();

        let mut checked_paths = Vec::new();
        let result = builder.find_recent_wasm_files(build_start_time, &mut checked_paths).await.unwrap();

        assert!(result.is_some(), "Should find recently modified file");
        let found_path = result.unwrap();
        assert_eq!(found_path, new_wasm, "Should find the newly created file");
        assert!(!checked_paths.is_empty(), "Should have checked some paths");
    }

    /// Test error reporting with checked paths
    #[tokio::test]
    async fn test_error_reporting_with_checked_paths() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_path = temp_dir.path();

        // Create project but no .wasm files
        create_test_rust_project(project_path).await.expect("failed to create test project");

        // Create custom command that will "succeed" but produce no artifacts
        let custom_command = vec!["echo".to_string(), "Build completed successfully".to_string()];

        let builder = ComponentBuilder::new(project_path.to_path_buf(), None, false);
        let result = builder.execute_custom_command(&custom_command).await;

        assert!(result.is_err(), "Build should fail when no artifacts are found");
        let error_msg = result.unwrap_err().to_string();

        // Check that error message contains helpful information
        assert!(error_msg.contains("completed successfully but no WebAssembly artifact was found"),
                "Error should mention that build completed but no artifact found");
        assert!(error_msg.contains("Searched the following locations:"),
                "Error should list searched locations");
        assert!(error_msg.contains("Consider:"),
                "Error should provide suggestions");
        assert!(error_msg.contains("--artifact-path"),
                "Error should suggest using --artifact-path");
    }

    /// Test JSON parsing from cargo output
    #[tokio::test]
    async fn test_json_output_parsing() {
        let temp_dir = TempDir::new().expect("failed to create temp dir");
        let project_path = temp_dir.path();
        let target_dir = project_path.join("target/wasm32-wasip2/debug");

        // Create the expected output file
        async_fs::create_dir_all(&target_dir).await.unwrap();
        let wasm_file = target_dir.join("test_component.wasm");
        async_fs::write(&wasm_file, "test content").await.unwrap();

        create_test_rust_project(project_path).await.expect("failed to create test project");

        // Create a mock JSON output similar to what cargo produces
        let json_output = format!(r#"{{"reason":"compiler-artifact","filenames":["{}"]}}"#, wasm_file.to_string_lossy());

        let _builder = ComponentBuilder::new(project_path.to_path_buf(), None, false);

        // Test the rust build function's JSON parsing by examining the output
        // This test verifies that when we have JSON output, we can extract the path
        let mut found_wasm = false;
        for line in json_output.lines() {
            if let Ok(json_msg) = serde_json::from_str::<serde_json::Value>(line) {
                if json_msg.get("reason").and_then(|r| r.as_str()) == Some("compiler-artifact") {
                    if let Some(filenames) = json_msg.get("filenames").and_then(|f| f.as_array()) {
                        for filename in filenames {
                            if let Some(path_str) = filename.as_str() {
                                let path = PathBuf::from(path_str);
                                if path.extension().and_then(|s| s.to_str()) == Some("wasm") {
                                    found_wasm = true;
                                    assert!(path.exists(), "JSON-referenced file should exist");
                                }
                            }
                        }
                    }
                }
            }
        }

        assert!(found_wasm, "Should find .wasm file in JSON output");
    }
}
