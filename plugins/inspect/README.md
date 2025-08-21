# Inspect Plugin for wash

A wash plugin that provides component inspection functionality, allowing you to inspect WebAssembly components both as a command and automatically after development sessions.

## What it does

The inspect plugin provides detailed analysis of WebAssembly components, showing their interfaces, exports, imports, and metadata. It can be used in two ways:

- **Command Mode**: Execute `wash plugin-inspect <component-path>` to inspect WebAssembly components on demand
- **AfterDev Hook**: Automatically inspects components when `wash dev` sessions end

## Building

To build the plugin from source:

```bash
cd plugins/inspect
wash build
```

This will create a WebAssembly component at `target/wasm32-wasip2/debug/inspect.wasm` (or `release/` if built with `--release`).

## Installation

Install the plugin into your wash environment:

```bash
wash plugin install target/wasm32-wasip2/debug/inspect.wasm
```

Or install directly from the build directory:

```bash
wash plugin install file://$(pwd)/target/wasm32-wasip2/debug/inspect.wasm
```

## Usage

### As a Command

```bash
# Inspect a local component file
wash plugin-inspect ./my-component.wasm

# Inspect a component at a specific path
wash plugin-inspect /path/to/component.wasm
```

### As an AfterDev Hook

The plugin automatically registers an `after-dev` hook that will run when `wash dev` sessions end. It will:

1. Look for the component artifact path in the development context
2. Inspect the component and display its interface information
3. Provide a summary of the component's exports, imports, and capabilities

This gives you immediate feedback about your component after each development iteration.

## Features

- **WIT Interface Display**: Shows WebAssembly Interface Types (WIT) and component interfaces
- **Export/Import Analysis**: Lists all component exports and imports
- **Component Metadata**: Displays component metadata and version information
- **Automatic Integration**: Works seamlessly with `wash dev` workflow
