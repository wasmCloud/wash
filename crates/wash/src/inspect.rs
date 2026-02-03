//! Common utilities for inspecting and decoding WIT components

use std::io::Read;

use anyhow::Context;
use wit_component::{DecodedWasm, OutputToString};

/// Decode Wasm from anything that implements `Read` into a [`DecodedWasm`].
pub async fn decode_component(component_bytes: impl Read) -> anyhow::Result<DecodedWasm> {
    wit_component::decode_reader(component_bytes).context("failed to decode component bytes")
}

/// Get the decoded WIT world from a component as a pretty-printed string.
pub async fn get_component_wit(component: DecodedWasm) -> anyhow::Result<String> {
    let resolve = component.resolve();
    let main = component.package();

    let mut printer = wit_component::WitPrinter::new(OutputToString::default());
    printer
        .print(resolve, main, &[])
        .context("failed to print WIT world from a component")?;

    Ok(printer.output.to_string())
}
