# HTTP Hello World in Rust

This is a simple Rust Wasm example that responds with a "Hello World" message for each request.

## Prerequisites

- [Wasm Shell (`wash`)](https://wasmcloud.com/docs/v2.0.0-rc/wash/) v2.0.0-rc.6
- `cargo` 1.82+

## Local development

To get started developing this example quickly, clone the repo and run `wash dev`:

```bash
wash dev
```

`wash dev` does many things for you:

- Builds this project
- Runs the component locally, exposing the application at `localhost:8000`
- Watches your code for changes and re-deploys when necessary.

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
wash build
```