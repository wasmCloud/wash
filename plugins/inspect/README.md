# Inspect Plugin for wash

A wash plugin that provides component inspection functionality, replicating the behavior of `wash inspect` but as a plugin that can be used in both command mode and as an automatic hook after development sessions.

## Features

- **Command Mode**: Execute `wash inspect <component-path>` to inspect WebAssembly components
- **AfterDev Hook**: Automatically inspects components when `wash dev` sessions end
- **WIT Interface Display**: Shows the WebAssembly Interface Types (WIT) of components

## Usage

### As a Command

```bash
# Inspect a local component file
wash inspect ./my-component.wasm

# Inspect a component at a specific path  
wash inspect /path/to/component.wasm
```

### As an AfterDev Hook

The plugin automatically registers an `AfterDev` hook that will run when `wash dev` sessions end. It will:

1. Look for the component artifact path in the development context
2. Fall back to common artifact locations if not found in context
3. Inspect the component and display its WIT interface
4. Provide a summary of the inspection results

## Implementation Status

⚠️ **Note**: This plugin is currently a proof-of-concept implementation with some limitations:

- **Synchronous Inspection**: The current plugin interface doesn't support async operations, so the full WIT parsing is not yet implemented
- **Local Files Only**: OCI reference support is not yet implemented (TODO)
- **Basic Output**: Full WIT formatting and display needs enhancement

## TODOs

- [ ] Implement full synchronous component inspection
- [ ] Add support for OCI references like the main `wash inspect` command
- [ ] Enhance WIT output formatting
- [ ] Add error handling for malformed components
- [ ] Add configuration options for output format
- [ ] Add caching for repeated inspections

## Development

To build the plugin:

```bash
cd plugins/inspect
cargo component build --release
```

The plugin will be built as a WebAssembly component that can be loaded by wash.

## Architecture

The plugin follows the standard wash plugin architecture:

- `src/lib.rs`: Core inspection logic (adapted from wash's inspect module)
- `src/plugin.rs`: Plugin interface implementation with metadata and command handling
- `src/bindings.rs`: WIT bindings generation

The plugin exports the `wasmcloud:wash/plugin` interface and can be used by wash's plugin system.