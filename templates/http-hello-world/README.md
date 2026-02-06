# HTTP Hello World in Rust

A minimal WebAssembly component built with [Rust](https://rust-lang.org/tools/install/) that responds to HTTP requests. The template uses [`wstd`](https://github.com/bytecodealliance/wstd), an async Rust standard library for Wasm components and WASI 0.2.

## Prerequisites

- [Wasm Shell (`wash`)](https://wasmcloud.com/docs/v2.0.0-rc/wash/) v2.0.0-rc.7
- [`cargo`](https://rust-lang.org/tools/install/) 1.82+

## Local development

Use `wash new` to scaffold a new wasmCloud component project:

```shell
wash new https://github.com/wasmCloud/wash.git --name http-hello-world --subfolder templates/http-hello-world
```

```shell
cd http-hello-world
```

To build this project and run in a hot-reloading development loop, run `wash dev` from this directory:

```bash
wash dev
```

### Send a request to the running component

Once `wash dev` is serving your component, send a request to the running component:

```bash
curl localhost:8000
```
```text
Hello from Rust!
```

## Build Wasm binary

```bash
cargo build --target wasm32-wasip2 --release
```