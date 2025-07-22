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

#[cfg(test)]
mod test {
    use std::fs::File;

    use crate::inspect::{decode_component, get_component_wit};

    #[tokio::test]
    async fn can_load_and_decode_component_reader() {
        let component_file = File::open("./tests/fixtures/http_hello_world_rust.wasm")
            .expect("should be able to open component file");
        let decoded = decode_component(&component_file)
            .await
            .expect("should be able to decode component");
        let wit = get_component_wit(decoded)
            .await
            .expect("should be able to print component WIT");
        eprintln!("{wit}");
    }
}
