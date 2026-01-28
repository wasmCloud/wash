# HTTP Hello World in Go

This is a simple Go (TinyGo) Wasm example that responds with a "Hello World" message for each request.

## Prerequisites

- [Wasm Shell (`wash`)](https://wasmcloud.com/docs/v2.0.0-rc/wash/) v2.0.0-rc.6
- [Go](https://go.dev/doc/install) 1.24.x
- [TinyGo](https://tinygo.org/getting-started/install/) (always use the latest version):
- [`wasm-tools`](https://github.com/bytecodealliance/wasm-tools#installation) **v1.225.0**
- [`make`](https://www.gnu.org/software/make/)

:::warning[]
Due to incompatibilities introduced in `wasm-tools` v1.226.0 and higher, **we strongly recommend using `wasm-tools` v1.225.0** when building Go projects. The easiest way to download `wasm-tools` v1.225.0 is via [cargo](https://doc.rust-lang.org/cargo/getting-started/installation.html): `cargo install --locked wasm-tools@1.225.0`
:::

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
Hello from Go!
```

## Build Wasm binary

```bash
wash build
```