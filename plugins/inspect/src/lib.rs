//! This plugin implements component inspection functionality similar to `wash inspect`
//! It can be used as a command-line plugin and includes an AfterDev hook to automatically
//! inspect components after the development loop ends.

use anyhow::Context;
use std::io::Read;
use wasi::filesystem::preopens;
use wasi::filesystem::types::{Descriptor, DescriptorFlags, OpenFlags, PathFlags};
use wit_component::DecodedWasm;

/// Generated WIT bindings
pub mod bindings;
/// `wasmcloud:wash/plugin` implementation
pub mod plugin;

/// Main component struct
struct Component;

/// Decode Wasm from bytes into a [`DecodedWasm`].
/// This replicates the functionality from wash's inspect module.
fn decode_component(component_bytes: &[u8]) -> anyhow::Result<DecodedWasm> {
    wit_component::decode_reader(component_bytes).context("failed to decode component bytes")
}

/// Get the decoded WIT world from a component as a pretty-printed string.
/// This replicates the functionality from wash's inspect module.
fn get_component_wit(component: DecodedWasm) -> anyhow::Result<String> {
    let resolve = component.resolve();
    let main = component.package();

    let mut printer = wit_component::WitPrinter::default();
    let output = printer
        .print(resolve, main, &[])
        .context("failed to print WIT world from a component")?;

    Ok(output)
}

/// Inspect a component and return its WIT interface as a string
pub fn inspect_component_bytes(component_bytes: &[u8]) -> anyhow::Result<String> {
    let component = decode_component(component_bytes).context("Failed to decode component")?;
    let wit = get_component_wit(component).context("Failed to get component WIT")?;
    Ok(wit)
}

/// Get the root directory of the filesystem
fn get_root_dir() -> anyhow::Result<(Descriptor, String)> {
    let dirs = preopens::get_directories();

    // Log all available directories for debugging
    for (i, (_, path)) in dirs.iter().enumerate() {
        println!("Preopened directory {}: {}", i, path);
    }

    if let Some(dir) = dirs.into_iter().next() {
        Ok((dir.0, dir.1))
    } else {
        anyhow::bail!("No preopened directories found")
    }
}

/// Read a file using wasi:filesystem
fn read_file_bytes(file_path: &str) -> anyhow::Result<Vec<u8>> {
    let (root_dir, root_path) = get_root_dir().context("Failed to get root directory")?;

    println!("Attempting to read file: {} from root: {}", file_path, root_path);

    let file = root_dir
        .open_at(
            PathFlags::empty(),
            file_path,
            OpenFlags::empty(),
            DescriptorFlags::READ,
        )
        .context(format!("Failed to open file: {file_path}"))?;

    let mut stream = file
        .read_via_stream(0)
        .context("Failed to create read stream")?;

    let mut buf = Vec::new();
    stream
        .read_to_end(&mut buf)
        .context("Failed to read file contents")?;

    Ok(buf)
}

/// Check if a file exists using wasi:filesystem
fn file_exists(file_path: &str) -> anyhow::Result<bool> {
    let (root_dir, _root_path) = get_root_dir().context("Failed to get root directory")?;

    match root_dir.stat_at(PathFlags::empty(), file_path) {
        Ok(_) => Ok(true),
        Err(e) => {
            if e == wasi::filesystem::types::ErrorCode::NoEntry {
                Ok(false)
            } else {
                anyhow::bail!("Failed to stat file {file_path}: {e}")
            }
        }
    }
}

/// Inspect a component and return its WIT interface as a string
pub fn inspect_component(component_path: &str) -> anyhow::Result<String> {
    let component_bytes = read_file_bytes(component_path)
        .context(format!("Failed to read component file: {component_path}"))?;

    let component = decode_component(&component_bytes).context("Failed to decode component")?;

    let wit = get_component_wit(component).context("Failed to get component WIT")?;

    Ok(wit)
}
