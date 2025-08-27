#![doc = include_str!("../../../README.md")]

/// Command line interface implementations for wash
pub mod cli;
/// Build Wasm components
pub mod component_build;
/// Configuration management for wash
pub mod config;
/// Implementations for the developer loop, including component plugin management
pub mod dev;
/// Component inspection and analysis
pub mod inspect;
/// Create new wash projects
pub mod new;
/// OCI registry operations for WebAssembly components
pub mod oci;
/// Plugin management for wash
pub mod plugin;
/// [`wasmcloud_runtime::Runtime`] management for wash
pub mod runtime;

/// Manage WebAssembly Interface Types (WIT) for wash components
pub(crate) mod wit;

/// TypeScript project utilities for dependency management and optimization
pub(crate) mod typescript_utils;

/// The current version of the wash package, set at build time
pub const CARGO_PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
