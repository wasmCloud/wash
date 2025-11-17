# wash - The Wasm Shell

[![Apache 2.0 License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![GitHub Release](https://img.shields.io/github/v/release/wasmcloud/wash)](https://github.com/wasmcloud/wash/releases)

**wash** is the comprehensive command-line tool for developing, building, and managing WebAssembly components. It provides an intuitive developer experience for the modern Wasm ecosystem, from project scaffolding to building and pushing components to OCI registries.

## Features

- **Project Creation**: Generate new WebAssembly component projects from templates
- **Multi-Language Build System**: Compile components for multiple languages (Rust, Go, TypeScript)
- **Development Loop**: Built-in hot-reload development server (`wash dev`)
- **OCI Registry Integration**: Push and pull components to/from OCI-compatible registries
- **Plugin System**: Extensible architecture with WebAssembly-based plugins
- **Component Inspection**: Analyze component WIT interfaces and metadata
- **Environment Health Checks**: Built-in diagnostics and system verification
- **Configuration Management**: Hierarchical configuration with global and project-level settings
- **Self-Updates**: Keep wash up-to-date with the latest features and fixes

## Installation

### Pre-built Binaries

Download the latest release for your platform from [GitHub Releases](https://github.com/wasmcloud/wash/releases).

### Install

Quick install (latest release)

**Linux/macOS:**

```bash
curl -fsSL https://raw.githubusercontent.com/wasmcloud/wash/refs/heads/main/install.sh | bash
```

**Windows (PowerShell):**

```powershell
iwr -useb https://raw.githubusercontent.com/wasmcloud/wash/refs/heads/main/install.ps1 | iex
```

Make sure to move `wash` to somewhere in your `PATH`.

### From Source

```bash
git clone https://github.com/wasmcloud/wash.git
cd wash
cargo install --path .
```

## Quick Start

1. **Check your environment:**

   ```bash
   wash doctor
   ```

2. **Create a new component:**

   ```bash
   wash new
   ```

3. **Build your component:**

   ```bash
   wash build ./http-hello-world
   ```

4. **Start a development loop**

   ```bash
   wash dev ./http-hello-world
   ```

5. **Keep wash updated:**

   ```bash
   wash update
   ```

## Commands

| Command           | Description                                                     |
| ----------------- | --------------------------------------------------------------- |
| `wash build`      | Build a Wasm component                                          |
| `wash config`     | View and manage wash configuration                              |
| `wash completion` | Generate shell completion scripts for wash                      |
| `wash dev`        | Start a development server for a Wasm component with hot-reload |
| `wash doctor`     | Check the health of your wash installation and environment      |
| `wash host`       | Act as a host.                                                  |
| `wash inspect`    | Inspect a Wasm component's embedded WIT interfaces              |
| `wash new`        | Create a new project from a template or git repository          |
| `wash oci`        | Push or pull Wasm components to/from an OCI registry            |
| `wash plugin`     | Manage wash plugins                                             |
| `wash update`     | Update wash to the latest version                               |
| `wash wit`        | Manage WIT dependencies                                         |
| `wash help`       | Print this message or the help of the given subcommand(s)       |

Run `wash --help` or `wash <command> --help` for detailed usage information.

### Plugin Commands

wash also supports custom commands through its plugin system. Plugins are automatically discovered and made available as subcommands.

## Plugin System

wash features an extensible plugin architecture built on WebAssembly:

- **Built-in Plugins**: oauth, blobstore-filesystem, aspire-otel
- **Platform Integration**: Plugins can integrate wash with specific platforms (like wasmCloud)
- **Custom Plugins**: Write your own plugins using the WebAssembly Component Model
- **Automatic Discovery**: Plugins in the `plugins/` directory are automatically loaded
- **Hook System**: Plugins can register pre and post-command hooks for workflow customization

Use `wash plugin --help` to see plugin management commands.

### Shell Completion

#### Zsh

For zsh completion, please run:

```shell
mkdir -p ~/.zsh/completion
wash completion zsh > ~/.zsh/completion/_wash
```

and put the following in `~/.zshrc`:

```shell
fpath=(~/.zsh/completion $fpath)
```

Note if you're not running a distribution like oh-my-zsh you may first have to enable autocompletion (and put in `~/.zshrc` to make it persistent):

```shell
autoload -Uz compinit && compinit
```

#### Bash

To enable bash completion, run the following, or put it in `~/.bashrc` or `~/.profile`:

```shell
. <(wash completion bash)
```

#### Fish

The below commands can be used for fish auto completion:

```shell
mkdir -p ~/.config/fish/completions
wash completion fish > ~/.config/fish/completions/wash.fish
```

#### Powershell

The below command can be referred for setting it up. Please note that the path might be different depending on your
system settings.

```shell
wash completion powershell > $env:UserProfile\\Documents\\WindowsPowerShell\\Scripts\\wash.ps1
```

## Architecture

wash is built with the following key principles:

- **Component-First**: Native support for the WebAssembly Component Model
- **Language Agnostic**: Support for Rust, Go (TinyGo), TypeScript, and more
- **OCI Compatible**: Components are stored and distributed using OCI registries
- **Portable Components**: Produces WebAssembly components that are runtime-agnostic and compatible with any Component Model runtime
- **Wasmtime-Powered**: Uses Wasmtime for local component execution and development workflows
- **Extensible**: Plugin system allows integration with different platforms and workflows
- **Developer Experience**: Hot-reload development loops and comprehensive tooling

## Documentation

- [WebAssembly Component Model](https://component-model.bytecodealliance.org/) - Learn about the component model
- [WASI Preview 2](https://github.com/WebAssembly/WASI/tree/main/preview2) - WebAssembly System Interface
- [wasmCloud Documentation](https://wasmcloud.com/docs) - Platform integration via plugins
- [Contributing Guide](CONTRIBUTING.md) - How to contribute to this project

## Support

- [GitHub Issues](https://github.com/wasmcloud/wash/issues) - Bug reports and feature requests
- [GitHub Discussions](https://github.com/wasmcloud/wash/discussions) - Community support and Q&A
- [WebAssembly Community](https://webassembly.org/community/) - Broader WebAssembly ecosystem

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.
