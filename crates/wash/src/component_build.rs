//! Component build configuration for different language toolchains

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use std::str::FromStr;

/// Build configuration for different language toolchains
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BuildConfig {
    /// Rust-specific build configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rust: Option<RustBuildConfig>,

    /// TinyGo-specific build configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tinygo: Option<TinyGoBuildConfig>,

    /// TypeScript-specific build configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typescript: Option<TypeScriptBuildConfig>,

    /// Expected path to the built Wasm component artifact
    #[serde(skip_serializing_if = "Option::is_none")]
    pub component_path: Option<PathBuf>,
}

/// Types of projects that can be built
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectType {
    /// Rust project (Cargo.toml found)
    Rust,
    /// Go project (go.mod found)
    Go,
    /// TypeScript/JavaScript project (package.json found)
    TypeScript,
    /// Unknown project type
    Unknown,
}

/// Rust-specific build configuration with explicit defaults
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RustBuildConfig {
    /// Custom build command that overrides all other Rust build settings
    /// When specified, all other Rust build flags are ignored
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_command: Option<Vec<String>>,

    /// Target architecture for Rust builds (default: wasm32-wasip2)
    #[serde(default = "default_rust_target")]
    pub target: String,

    /// Additional cargo flags (default: empty)
    #[serde(default)]
    pub cargo_flags: Vec<String>,

    /// Release mode (default: false)
    #[serde(default)]
    pub release: bool,

    /// Features to enable (default: empty)
    #[serde(default)]
    pub features: Vec<String>,

    /// Build with no default features (default: false)
    #[serde(default)]
    pub no_default_features: bool,
}

impl Default for RustBuildConfig {
    fn default() -> Self {
        Self {
            custom_command: None,
            target: default_rust_target(),
            cargo_flags: Vec::new(),
            release: false,
            features: Vec::new(),
            no_default_features: false,
        }
    }
}

fn default_rust_target() -> String {
    "wasm32-wasip2".to_string()
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TinyGoScheduler {
    #[default]
    Asyncify,
    Tasks,
    None,
    Other(String), // For future extensions
}

impl std::fmt::Display for TinyGoScheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            TinyGoScheduler::Asyncify => "asyncify",
            TinyGoScheduler::Tasks => "tasks",
            TinyGoScheduler::None => "none",
            TinyGoScheduler::Other(s) => s,
        };
        write!(f, "{s}")
    }
}

impl FromStr for TinyGoScheduler {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "asyncify" => Ok(TinyGoScheduler::Asyncify),
            "tasks" => Ok(TinyGoScheduler::Tasks),
            "none" => Ok(TinyGoScheduler::None),
            other => Ok(TinyGoScheduler::Other(other.to_string())),
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TinyGoGarbageCollector {
    #[default]
    Conservative,
    Leaking,
    None,
    Custom(String), // For future extensions
}

impl std::fmt::Display for TinyGoGarbageCollector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            TinyGoGarbageCollector::Conservative => "conservative",
            TinyGoGarbageCollector::Leaking => "leaking",
            TinyGoGarbageCollector::None => "none",
            TinyGoGarbageCollector::Custom(s) => s,
        };
        write!(f, "{s}")
    }
}

impl FromStr for TinyGoGarbageCollector {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "conservative" => Ok(TinyGoGarbageCollector::Conservative),
            "leaking" => Ok(TinyGoGarbageCollector::Leaking),
            "none" => Ok(TinyGoGarbageCollector::None),
            other => Ok(TinyGoGarbageCollector::Custom(other.to_string())),
        }
    }
}

/// TinyGo-specific build configuration with explicit defaults  
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TinyGoBuildConfig {
    /// Custom build command that overrides all other TinyGo build settings
    /// When specified, all other TinyGo build flags are ignored
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_command: Option<Vec<String>>,

    /// TinyGo target (default: wasip2)
    #[serde(default = "default_tinygo_target")]
    pub target: String,

    /// Additional build flags (default: empty)
    #[serde(default)]
    pub build_flags: Vec<String>,

    /// Disable the go generate during TinyGo build
    #[serde(default)]
    pub disable_go_generate: bool,

    /// The TinyGo scheduler to use (default: "asyncify")
    #[serde(default = "default_tinygo_scheduler")]
    pub scheduler: TinyGoScheduler,

    /// TinyGo garbage collector to use (default: conservative)
    #[serde(default = "default_tinygo_gc")]
    pub gc: TinyGoGarbageCollector,

    /// Optimization level (default: "z")
    #[serde(default = "default_tinygo_opt")]
    pub opt: String,

    /// Panic strategy (default: "print")
    #[serde(default = "default_tinygo_panic")]
    pub panic: String,

    /// Build tags (default: empty)
    #[serde(default)]
    pub tags: Vec<String>,

    /// Strip debug information (default: true)
    #[serde(default = "default_tinygo_no_debug")]
    pub no_debug: bool,

    /// Goroutine stack size (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack_size: Option<String>,

    /// The WIT package to use for TinyGo builds, if not provided
    /// it will assume only one WIT package is defined in the project
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wit_package: Option<String>,

    /// The WIT world to use for TinyGo builds, if not provided
    /// it will assume only one world is defined in the WIT package
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wit_world: Option<String>,
}

impl Default for TinyGoBuildConfig {
    fn default() -> Self {
        Self {
            custom_command: None,
            target: default_tinygo_target(),
            build_flags: Vec::new(),
            disable_go_generate: false,
            scheduler: default_tinygo_scheduler(),
            gc: default_tinygo_gc(),
            opt: default_tinygo_opt(),
            panic: default_tinygo_panic(),
            tags: Vec::new(),
            no_debug: default_tinygo_no_debug(),
            stack_size: None,
            wit_package: None,
            wit_world: None,
        }
    }
}

fn default_tinygo_target() -> String {
    "wasip2".to_string()
}

fn default_tinygo_scheduler() -> TinyGoScheduler {
    TinyGoScheduler::Asyncify
}

fn default_tinygo_gc() -> TinyGoGarbageCollector {
    TinyGoGarbageCollector::Conservative
}

fn default_tinygo_opt() -> String {
    "z".to_string()
}

fn default_tinygo_panic() -> String {
    "print".to_string()
}

fn default_tinygo_no_debug() -> bool {
    true
}

/// TypeScript-specific build configuration with explicit defaults
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeScriptBuildConfig {
    /// Custom build command that overrides all other TypeScript build settings
    /// When specified, all other TypeScript build flags are ignored
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_command: Option<Vec<String>>,

    /// Node.js version for building (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_version: Option<String>,

    /// Package manager to use (default: npm)
    #[serde(default = "default_ts_package_manager")]
    pub package_manager: String,

    /// Build command to pass to `npm run` (default: "build")
    #[serde(default = "default_ts_build_command")]
    pub build_command: String,

    /// Additional build flags (default: empty)
    #[serde(default)]
    pub build_flags: Vec<String>,

    /// Skip the installation of dependencies (default: false)
    #[serde(default)]
    pub skip_install: bool,

    /// Enable source maps (default: false)
    #[serde(default)]
    pub source_maps: bool,
}

impl Default for TypeScriptBuildConfig {
    fn default() -> Self {
        Self {
            custom_command: None,
            node_version: None,
            package_manager: default_ts_package_manager(),
            build_command: default_ts_build_command(),
            build_flags: Vec::new(),
            skip_install: false,
            source_maps: false,
        }
    }
}

fn default_ts_package_manager() -> String {
    "npm".to_string()
}

fn default_ts_build_command() -> String {
    "build".to_string()
}
