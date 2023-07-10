# Developer guide

This document serves as a guide and reference for people looking to develop `wash`.

- [Developer guide](#developer-guide)
  - [Development Prerequistes](#development-prerequistes)
    - [`build` Integration Tests](#build-integration-tests)
    - [Dependency Check Script](#dependency-check-script)
    - [Optional Tools](#optional-tools)
  - [Building the project](#building-the-project)
  - [Testing the project](#testing-the-project)
  - [Making Commits](#making-commits)

## Development Prerequistes

To contribute to `wash`, you just need [Rust](https://rustup.rs/) installed.

Due to the use of the as-of-yet unstable [`bindeps` feature][bindeps], `wash` also requires [nightly rust][rust-nightly] to
be installed. `wash` uses nightly rust to build the [WASI preview1 adapter component][wasi-p1-adapter].

To run any `wash` tests, you need to install [`nextest`](https://nexte.st/index.html). With a Rust toolchain already installed, you can simply install this with:

```bash
cargo install cargo-nextest --locked
```

The dependency check script will also install this for you, see that section below.

[bindeps]: https://github.com/rust-lang/cargo/issues/9096
[nightly-rust]: https://rust-lang.github.io/rustup/concepts/channels.html#working-with-nightly-rust
[wasi-p1-adapter]: https://github.com/bytecodealliance/wasmtime/tree/main/crates/wasi-preview1-component-adapter

### `build` Integration Tests

To run the `wash build` integration tests that compile actors using actual language toolchains, you must have those toolchains installed. Currently the requirements for this are:

- [Rust](https://rustup.rs/)
  - The `wasm32-unknown-unknown` target must be installed.
    - You can install this with: `rustup target add wasm32-unknown-unknown`.
- [TinyGo](https://tinygo.org/getting-started/install/)
  - TinyGo also requires [Go](https://go.dev/doc/install) to be installed.

### Dependency Check Script

To make it easy to ensure you have all the right tools installed to run all of the `wash` tests, we've created a Python script at `tools/deps_check.py`. You can run this using `make deps-check` or `python3 ./tools/deps_check.py`.

### Optional Tools

While developing `wash`, consider installing the following optional development tools:

- [`cargo-watch`](https://crates.io/crates/cargo-watch) (`cargo install cargo-watch`) to enable the `*-watch` commands

These will be automatically installed using the `deps_check.py` script as well.

## Building the project

To build the project:

```console
make build
```

To build continuously (thanks to [`cargo-watch`](https://crates.io/crates/cargo-watch)):

```console
make build-watch
```

### Building the WASI preview1 component adapters

If you find it necessary to rebuild the preview1 components adapters, you must use `git submodule` (as [`wasmtime`][wasmtime] does). After checking out this repository:

```console
git submodule update --init # initialize the vendor/wasmtime submodule
cd vendor/wasmtime
git submodule update --init # intiialize the submodules inside wasmtime
curl -L https://github.com/bytecodealliance/wasm-tools/releases/download/wasm-tools-1.0.27/wasm-tools-1.0.27-x86_64-linux.tar.gz | tar xfz -
export PATH=`pwd`/wasm-tools-1.0.27-x86_64-linux:$PATH
./ci/build-wasi-preview1-component-adapter.sh
mv target/wasm32-unknown-unknown/release/wasi_* ../wasm/
```

After performing the steps above you should have both adapters at the following paths inside the respository:

- `vendor/wasmtime/target/wasm32-unknown-unknown/release/wasi_snapshot_preview1.command.wasm`
- `vendor/wasmtime/target/wasm32-unknown-unknown/release/wasi_snapshot_preview1.reactor.wasm`

The files listed above should be identical to the ones at `vendor/wasm/wasi_snapshot_preview1.(command|reactor).wasm`.

[wasmtime]: https://github.com/bytecodealliance/wasmtime

## Testing the project

To test all unit tests:

```console
make test
```

To test all unit tests continuously:

```console
make test-watch
```

To test a *specific* target test(s) continuously:

```console
TARGET=integration_new_handles_dashed_names make test-watch
```

## Making Commits

For us to be able to merge in any commits, they need to be signed off. If you didn't do so, the PR bot will let you know how to fix it, but it's worth knowing how to do it in advance.

There are a few options:
- use `git commit -s` in the CLI
- in `vscode`, go to settings and set the `git.alwaysSignOff` setting. Note that the dev container configuration in this repo sets this up by default.
- manually add "Signed-off-by: NAME <EMAIL>" at the end of each commit

You may also be able to use GPG signing in leu of a sign off.
