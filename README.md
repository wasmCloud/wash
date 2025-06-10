# wash - The Wasm Shell

> ⚠️ **Experimental**  
> This repository is experimental and under active development. Breaking changes are likely as the project evolves.

[![Apache 2.0 License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)
[![GitHub Release](https://img.shields.io/github/v/release/cosmonic-labs/wash)](https://github.com/cosmonic-labs/wash/releases)

**wash** is the command-line tool for developing, building, and managing WebAssembly components. It provides an intuitive developer experience for the Wasm ecosystem, from project scaffolding to building and pushing components to OCI registries.

## Features

- **Project Creation**: Generate new WebAssembly component projects from templates
- **Build System**: Compile components for multiple languages (Rust, Go, TypeScript)
- **Development Tools**: Built-in environment health checking and diagnostics
- **Configuration Management**: Manage wash settings and project configurations
- **Self-Updates**: Keep wash up-to-date with the latest features and fixes

## Installation

### Pre-built Binaries

Download the latest release for your platform from [GitHub Releases](https://github.com/cosmonic-labs/wash/releases).

### Install

Quick install (latest release)

```bash
curl -fsSL https://raw.githubusercontent.com/cosmonic-labs/wash/refs/heads/main/install.sh | bash
```

Make sure to move `wash` to somewhere in your `PATH`.

### From Source

```bash
git clone https://github.com/cosmonic-labs/wash.git
cd wash
cargo build --release
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

| Command       | Description                                                |
| ------------- | ---------------------------------------------------------- |
| `wash build`  | Build a Wasm component                                     |
| `wash config` | View configuration for wash                                |
| `wash dev`    | Start a development server for a Wasm component            |
| `wash doctor` | Check the health of your wash installation and environment |
| `wash new`    | Create a new project from a template or git repository     |
| `wash oci`    | Push or pull Wasm components to/from an OCI registry       |
| `wash update` | Update wash to the latest version                          |
| `wash help`   | Print this message or the help of the given subcommand(s)  |

Run `wash --help` or `wash <command> --help` for detailed usage information.

## Documentation

- [Cosmonic Developer Guide](https://cosmonic.com/docs/developer-guide/developing-webassembly-components) - Develop components for Cosmonic
- [WebAssembly Component Model](https://component-model.bytecodealliance.org/) - Learn about the component model
- [WASI Preview 2](https://github.com/WebAssembly/WASI/tree/main/preview2) - WebAssembly System Interface
- [Contributing Guide](CONTRIBUTING.md) - How to contribute to this project

## Support

- [GitHub Issues](https://github.com/cosmonic-labs/wash/issues) - Bug reports and feature requests
- [GitHub Discussions](https://github.com/cosmonic-labs/wash/discussions) - Community support and Q&A
- [WebAssembly Community](https://webassembly.org/community/) - Broader WebAssembly ecosystem

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.
