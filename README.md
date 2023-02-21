[![Latest Release](https://img.shields.io/github/v/release/wasmcloud/wash?color=success&include_prereleases)](https://github.com/wasmCloud/wash/releases)
[![Rust Build](https://img.shields.io/github/actions/workflow/status/wasmcloud/wash/rust_ci.yml?branch=main)](https://github.com/wasmCloud/wash/actions/workflows/rust_ci.yml)
[![Rust Version](https://img.shields.io/badge/rustc-1.66.0-orange.svg)](https://blog.rust-lang.org/2022/12/15/Rust-1.66.0.html)
[![Contributors](https://img.shields.io/github/contributors/wasmcloud/wash)](https://github.com/wasmCloud/wash/graphs/contributors)
[![Good first issues](https://img.shields.io/github/issues/wasmcloud/wash/good%20first%20issue?label=good%20first%20issues)](https://github.com/wasmCloud/wash/issues?q=is%3Aopen+is%3Aissue+label%3A%22good+first+issue%22)
[![wash-cli](https://img.shields.io/crates/v/wash-cli)](https://crates.io/crates/wash-cli)

```
                                     _                 _    _____ _          _ _
                                ____| |               | |  / ____| |        | | |
 __      ____ _ ___ _ __ ___  / ____| | ___  _   _  __| | | (___ | |__   ___| | |
 \ \ /\ / / _` / __| '_ ` _ \| |    | |/ _ \| | | |/ _` |  \___ \| '_ \ / _ \ | |
  \ V  V / (_| \__ \ | | | | | |____| | (_) | |_| | (_| |  ____) | | | |  __/ | |
   \_/\_/ \__,_|___/_| |_| |_|\_____|_|\___/ \__,_|\__,_| |_____/|_| |_|\___|_|_|
```

`wash` is the comprehensive CLI to [wasmCloud][wasmcloud]. `wash` helps you:

- [Start wasmCloud](#wash-up)
- [Generate new wasmCloud projects](#wash-new)
- [Push and pull from OCI compliant registries](#wash-reg)
- [Interact directly with a wasmCloud instance](#wash-ctl)
- [Manage cryptographic signing keys](./docs/cli/README.md#wash-claims)

`wash` is a single binary that makes developing WebAssembly with wasmCloud painless and simple.

## Getting started

<details open>
<summary>🦀 Cargo</summary>

The easiest way to get started with `wash` is via [`cargo`][cargo]

```console
cargo install wash-cli
```
</details>

<details>
<summary>🐧 Linux (deb/rpm + apt)</summary>

```console
# Debian / Ubuntu (deb)
curl -s https://packagecloud.io/install/repositories/wasmcloud/core/script.deb.sh | sudo bash
# Fedora (rpm)
curl -s https://packagecloud.io/install/repositories/wasmcloud/core/script.rpm.sh | sudo bash

sudo apt install wasmcloud wash
```
</details>

<details>
<summary>🐧 Linux (snap)</summary>

```console
sudo snap install wash --edge --devmode
```
</details>

<details>
<summary>🍎 MacOS (brew)</summary>

```console
brew tap wasmcloud/wasmcloud
brew install wasmcloud wash
```
</details>

<details>
<summary>🪟 Windows (choco)</summary>

```powershell
choco install wash
```
</details>

<details>
<summary>❄️ NixOS</summary>

```console
nix run github:wasmCloud/wash
```
</details>

## Using `wash`

`wash` provides subcommands for usage and development of wasmCloud.

### `wash up`

Bootstrap a [wasmCloud][wasmcloud-otp] environment in one easy command.

`wash up` supports both launching NATS and wasmCloud in the background as well as an "interactive" mode for shorter lived hosts.

After you run `wash up`, visit the [wasmCloud dashboard][washboard] at `http://localhost:4000`.

[washboard]: https://wasmcloud.com/docs/getting-started#viewing-the-wasmcloud-dashboard

### `wash new`

Create new wasmCloud projects from [predefined templates](https://github.com/wasmCloud/project-templates).

This command is a one-stop-shop for creating new actors, providers, and interfaces for all aspects of your application.

### `wash reg`

Push and Pull actors and capability providers to/from OCI compliant registries.

Useful for local development and CI/CD and in local development, with a local container/artifact registry.

### `wash ctl`

Interact directly with a wasmCloud [control-interface](https://github.com/wasmCloud/control-interface), allowing you to imperatively schedule actors, providers and modify configurations of a wasmCloud host. Can be used to interact with local and remote control-interfaces.

### `wash ctx`

Automatically connect to your previously launched wasmCloud lattice with a managed context or use contexts to administer remote wasmCloud lattices.

### ...and much more

For the full listing of commands [see the CLI documentation](./docs/cli/README.md).

## Contributing to `wash`

Feature suggestions? Find a bug? Have a question? [Submit an issue](https://github.com/wasmcloud/wash/issues/new/choose).

Contributions of all kinds (filling issues, creating pull requests) are welcome, and the [good first issue label](https://github.com/wasmcloud/wash/issues?q=is%3Aopen+is%3Aissue+label%3A%22good+first+issue%22) is a great place to start.

[cargo]: https://doc.rust-lang.org/cargo/
[wasmcloud]: https://wasmcloud.com
[wasmcloud-otp]: https://github.com/wasmCloud/wasmcloud-otp
