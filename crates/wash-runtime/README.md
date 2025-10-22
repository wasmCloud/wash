# wash-runtime

[![Apache 2.0 License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](../../LICENSE)

**wash-runtime** is an opinionated Wasmtime wrapper that provides a runtime and workload API for executing WebAssembly components. It offers a simplified interface for embedding Wasm component execution in Rust applications with built-in support for WASI interfaces.

## Features

- **Component Model Runtime**: Native support for WebAssembly Component Model using Wasmtime
- **WASI Interface Support**: Built-in plugins for WASI HTTP, Config, Logging, Blobstore, and Key-Value
- **Workload API**: High-level API for managing and executing component workloads
- **Plugin System**: Extensible architecture for custom capability providers
- **OCI Integration**: Optional support for pulling components from OCI registries
- **Hot-Reload Ready**: Designed for development workflows with fast iteration

## Usage

### Basic Example

```rust
use std::sync::Arc;
use std::collections::HashMap;

use wash_runtime::{
    engine::Engine,
    host::{HostBuilder, HostApi},
    plugin::{
        wasi_config::RuntimeConfig,
        wasi_http::HttpServer,
    },
    types::{WorkloadStartRequest, Workload},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create a Wasmtime engine
    let engine = Engine::builder().build()?;

    // Configure plugins
    let http_plugin = HttpServer::new("127.0.0.1:8080".parse()?);
    let runtime_config_plugin = RuntimeConfig::default();

    // Build and start the host
    let host = HostBuilder::new()
        .with_engine(engine)
        .with_plugin(Arc::new(http_plugin))?
        .with_plugin(Arc::new(runtime_config_plugin))?
        .build()?;

    let host = host.start().await?;

    // Start a workload
    let req = WorkloadStartRequest {
        workload: Workload {
            namespace: "test".to_string(),
            name: "test-workload".to_string(),
            annotations: HashMap::new(),
            service: None,
            components: vec![],
            host_interfaces: vec![],
            volumes: vec![],
        },
    };

    host.workload_start(req).await?;

    Ok(())
}
```

### Cargo Features

The crate supports the following cargo features:

- `wasi-http` (default): HTTP client and server support via `wasmtime-wasi-http`
- `wasi-config` (default): Runtime configuration interface
- `wasi-logging` (default): Logging interface
- `wasi-blobstore` (default): Blob storage interface
- `wasi-keyvalue` (default): Key-value storage interface
- `oci`: OCI registry integration for pulling components

### Architecture

wash-runtime provides three main abstractions:

1. **Engine**: Wasmtime configuration and component compilation
2. **Host**: Runtime environment with plugin management
3. **Workload**: High-level API for managing component lifecycles

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](../../LICENSE) file for details.
