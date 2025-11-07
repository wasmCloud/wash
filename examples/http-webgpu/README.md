# Sample: `wasi:http` with `wasi:webgpu` in Rust

An example project showing how to build a spec-compliant
[`wasi:http/incoming-handler`][wasi-http] server for WASI 0.2 written in Rust that uses [WebGPU][wasi-webgpu]. This sample includes several [routes](#routes) that showcase different HTTP behavior, with the home route demonstrating WebGPU initialization.

## WebGPU Integration

This example demonstrates how to use the `wasi:webgpu` interface in a Wasm component. The home route (`/`) initializes a WebGPU instance, requests an adapter, and creates a device and queue:

```rust
let instance = wgpu::Instance::new(Default::default());
let adapter = instance.request_adapter(&Default::default()).await.unwrap();
let (device, queue) = adapter.request_device(&Default::default(), None).await.unwrap();
```

## Routes

The following HTTP routes are available from the component:

```text
/               # Hello world (with WebGPU initialization)
/wait           # Sleep for one second
/echo           # Echo the HTTP body
/echo-headers   # Echo the HTTP headers
/echo-trailers  # Echo the HTTP trailers
```

## Local Development

### Prerequisites

First, build `wash` with WebGPU support enabled:

```bash
$ cd /path/to/wash
$ cargo build --package wash --features wasi-webgpu
```

### Running the Example

To run this example locally with `wash dev`:

```bash
$ cd examples/http-webgpu
$ /path/to/wash/target/debug/wash dev
```

This will:
1. Build the component
2. Start a local HTTP server with WebGPU support
3. Watch for file changes and automatically rebuild

You can then make requests to the server:

```bash
$ curl http://localhost:8000/
Hello, wasi:http/proxy world!
```

The WebGPU initialization will be logged to the console showing the device and queue information:

```
device: Device { ... }
queue: Queue { ... }
```

## Building

To build the component manually:

```bash
$ cargo component build --release
```

The compiled Wasm component will be available at `target/wasm32-wasip2/release/sample_wasi_http_rust.wasm`.

## Requirements

- Rust toolchain with `wasm32-wasip2` target
- `cargo-component` for building Wasm components
- `wash` built with the `wasi-webgpu` feature enabled
  ```bash
  cargo build --package wash --features wasi-webgpu
  ```

## License

Apache-2.0 with LLVM Exception

[wasi-http]: https://github.com/WebAssembly/wasi-http
[wasi-webgpu]: https://github.com/WebAssembly/wasi-webgpu
