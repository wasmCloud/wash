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
    host::{HostBuilder, HostApi,
      http::{HttpServer, DynamicRouter},
  },
    plugin::{
        wasi_config::DynamicConfig,
    },
    types::{WorkloadStartRequest, Workload},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create a Wasmtime engine
    let engine = Engine::builder().build()?;

    // Configure plugins
    let http_router = DynamicRouter::default();
    let http_handler = HttpServer::new(http_router, "127.0.0.1:8080".parse()?).await?;
    let wasi_config_plugin = DynamicConfig::default();

    // Build and start the host
    let host = HostBuilder::new()
        .with_engine(engine)
        // if a handler is not provided, a 'deny all' implementation
        // will be used for outgoing http requests
        .with_http_handler(Arc::new(http_handler))
        .with_plugin(Arc::new(wasi_config_plugin))?
        .build()?;

    let host = host.start().await?;

    // Start a workload
    let req = WorkloadStartRequest {
        workload_id: uuid::Uuid::new_v4().to_string(),
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
- `wasi-webgpu` WebGPU interface
- `oci`: OCI registry integration for pulling components

### Architecture

wash-runtime provides three main abstractions:

1. **Engine**: Wasmtime configuration and component compilation
2. **Host**: Runtime environment with plugin management
3. **Workload**: High-level API for managing component lifecycles

## Workload Anatomy

Workloads are the fundamental composition unit in wash. They define what your WebAssembly components need, and the runtime handles the rest—matching capabilities, binding plugins, and wiring everything together automatically.

A Workload bundles everything the runtime needs to execute your components:

```
Workload
├── namespace / name / annotations    # Identity and metadata
├── Service?                          # Long-running process (wasi:cli/run)
├── Components[]                      # Pooled, invocable units
├── Volumes[]                         # HostPath | EmptyDir
└── host_interfaces[]                 # WIT interfaces to satisfy
```

The key insight: **components declare what they need via WIT interfaces, and the runtime provides implementations**. Your component imports `wasi:blobstore`—it doesn't care whether that's backed by the filesystem, memory, or NATS. The runtime finds a matching plugin and binds it.

Services are long-running processes that implement `wasi:cli/run`. Components are pooled and invoked on-demand—perfect for request handlers. Each can have its own `LocalResources` defining memory limits, environment variables, configuration, and volume mounts.

Volumes come in two flavors: `HostPath` mounts a directory from the host filesystem, while `EmptyDir` creates ephemeral storage that lives only as long as the workload.

## Component Linking

Components within a workload can call each other through WIT interfaces. If component A exports an interface and component B imports it, the runtime automatically wires them together.

```
Workload
├── http-handler (Component)
│   └── imports: myapp:auth/validator
├── auth-service (Component)
│   └── exports: myapp:auth/validator
└── [runtime links http-handler → auth-service]
```

Services can also call components. A long-running service might delegate work to pooled components, letting the service maintain state while components handle individual operations.

### Constraints

- **No circular dependencies** — If A imports from B, B cannot import from A (directly or transitively)
- **Single implementation** — Only one component can export a given interface within a workload
- **Same workload** — WIT calls are always local. Use messaging or HTTP for cross-workload calls

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](../../LICENSE) file for details.
