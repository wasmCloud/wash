# Wasmtime & wash-wasi Upgrade Guide

This guide documents the process for upgrading wasmtime dependencies and syncing wash-wasi with upstream wasmtime-wasi.

## Overview

The `wash-wasi` crate is a fork of `wasmtime-wasi` with minimal modifications. The goal is to keep it as close to upstream as possible, with only import path changes (`wasmtime_wasi` → `wash_wasi`).

## wash-wasi Specific Modifications

**IMPORTANT:** wash-wasi has custom loopback socket implementations that don't exist in upstream. These files require careful manual merging:

### Files with Custom Modifications (DO NOT blindly sync)

| File | wash-wasi Addition |
|------|-------------------|
| `src/sockets/loopback/` | Entire directory is wash-specific |
| `src/sockets/mod.rs` | Loopback module declaration |
| `src/sockets/tcp.rs` | `TcpSocket` enum with `Network`, `Loopback`, `Unspecified` variants + p3 wrapper methods |
| `src/sockets/udp.rs` | `UdpSocket` enum with `Network`, `Loopback`, `Unspecified` variants |
| `src/p2/tcp.rs` | `LoopbackInputStream`, `LoopbackOutputStream` implementations |
| `src/p2/udp.rs` | `LoopbackIncomingDatagramStream`, `LoopbackOutgoingDatagramStream` |
| `src/p2/host/tcp.rs` | Loopback handling in host implementations |
| `src/p2/host/udp.rs` | Loopback handling in host implementations |
| `src/p3/sockets/host/types/tcp.rs` | Uses loopback-aware socket APIs, bind/listen/accept with loopback parameter |
| `src/p3/sockets/host/types/udp.rs` | Uses loopback-aware socket APIs, bind/connect with loopback parameter |

### Files that CAN be Synced Directly

These files have no wash-specific modifications (only doc comment path changes):
- `src/cli.rs`, `src/cli/*.rs` - Only doc path changes and `#![allow(unsafe_code)]`
- `src/clocks.rs` - Only API updates, no wash customizations
- `src/ctx.rs` - Only doc path changes
- `src/filesystem.rs` - Only doc path changes
- `src/random.rs` - Only doc path changes
- `src/view.rs` - Only doc path changes
- `src/lib.rs` - Only module declarations
- `src/p1.rs` - Only doc path changes
- `src/p2/bindings.rs` - Bindgen config (update carefully)
- `src/p2/mod.rs` - Module structure
- `src/p2/host/clocks.rs` - No wash customizations
- `src/p2/host/filesystem.rs` - No wash customizations

## Pre-Upgrade Checklist

1. **Check wasmtime release notes** for breaking changes
2. **Review the changelog** at <https://github.com/bytecodealliance/wasmtime/blob/main/CHANGELOG.md>
3. **Identify API changes** that may require code updates

## Upgrade Steps

### 1. Update Cargo.toml Dependencies

Update wasmtime version in `Cargo.toml`:

```toml
wasmtime = { version = "NEW_VERSION", ... }
```

### 2. Sync Source Files that Can Be Copied Directly

For files without wash-specific modifications, copy from upstream and change imports:

```bash
# Find upstream path
ls ~/.cargo/git/checkouts/wasmtime-*/

# Example: Sync all p3 files
UPSTREAM=~/.cargo/git/checkouts/wasmtime-ae46461068d65b15/*/crates/wasi/src
WASH=./crates/wasi/src

# Copy and sed to change import paths
cp $UPSTREAM/p3/bindings.rs $WASH/p3/bindings.rs
sed -i '' 's/wasmtime_wasi/wash_wasi/g' $WASH/p3/bindings.rs
```

### 3. Manually Merge Socket Files

For files with loopback modifications, you must manually merge:

1. Diff the file to see upstream changes
2. Apply upstream API changes while preserving loopback code
3. Test thoroughly

```bash
# View diff for a socket file
diff -u $WASH/sockets/tcp.rs $UPSTREAM/sockets/tcp.rs
```

### 4. Sync Test Files

The test files should be exact copies of upstream with only import path changes.

```bash
diff -ru ./crates/wasi/tests $UPSTREAM/../tests
```

**Files to sync (in `crates/wasi/tests/all/`):**

- `main.rs` - Usually identical
- `store.rs` - Change `wasmtime_wasi` → `wash_wasi`
- `p1.rs` - Change `wasmtime_wasi` → `wash_wasi`
- `p2/sync.rs` - Change `wasmtime_wasi` → `wash_wasi`
- `p2/async_.rs` - Change `wasmtime_wasi` → `wash_wasi`
- `p2/api.rs` - Change `wasmtime_wasi` → `wash_wasi` (including in `bindgen!` macro)
- `p3/mod.rs` - Change `wasmtime_wasi` → `wash_wasi`
- `process_stdin.rs` - Change `wasmtime_wasi` → `wash_wasi`

### 5. Common API Changes to Watch For

#### WIT File Consolidation (v41+)

In v41, WIT dependency files were consolidated from directory structures to single files:

```
# Old structure (v38)
src/p3/wit/deps/clocks/monotonic-clock.wit
src/p3/wit/deps/clocks/wall-clock.wit
src/p3/wit/deps/clocks/world.wit

# New structure (v41)
src/p3/wit/deps/clocks.wit  # Single consolidated file
```

When upgrading, sync the entire `wit/deps/` directory from upstream.

#### Bindgen Macro Changes

- **Separator changes**: Watch for changes like `/` → `.` in bindgen paths
- **New options**: Check for new required or deprecated options
- **Ownership patterns**: The `ownership` setting may change

Example from v38 → v41:

```rust
// Old (v38)
trappable_imports: true,

// New (v41)
// removed - now default behavior
```

#### Async Trait Removal

Wasmtime has been removing `#[async_trait]` in favor of native async traits:

```rust
// Old
#[async_trait::async_trait]
impl SomeTrait for MyType { ... }

// New
impl SomeTrait for MyType { ... }
```

#### Module Path Changes

Watch for reorganized module paths in imports.

#### Test Artifact Naming

Test artifact constants may change naming conventions:

```rust
// Old
PREVIEW1_*, PREVIEW2_*

// New (v41+)
P1_*, P2_*, P3_*
```

#### Stub Functions for External Tests

Tests handled by other crates (wasi-cli, wasi-http) need stub functions:

```rust
#[expect(
    dead_code,
    reason = "tested in the wasi-cli crate, satisfying foreach_api! macro"
)]
fn p1_cli_much_stdout() {}
```

#### P3 Command Instantiation Changes (v41+)

The P3 Command API changed significantly:

```rust
// Old (v38)
let (command, instance) = Command::new(&mut store, &component, &linker).await?;
instance.run_concurrent(async move |store| {
    command.wasi_cli_run().call_run(store).await
}).await?

// New (v41)
let command = Command::instantiate_async(&mut store, &component, &linker).await?;
store.run_concurrent(async move |store| {
    command.wasi_cli_run().call_run(store).await
}).await?
```

#### P3 Socket API Changes (v41+)

Host trait methods moved and signatures changed:

```rust
// Old - bind was in HostTcpSocketWithStore
async fn bind<T>(store: &Accessor<T, Self>, ...) -> SocketResult<()>

// New - bind is in HostTcpSocket
async fn bind(&mut self, socket: Resource<TcpSocket>, ...) -> SocketResult<()>

// Old - listen signature
async fn listen<T>(store: &Accessor<T, Self>, ...) -> SocketResult<...>

// New - listen signature uses Access type
fn listen<T: 'static>(mut store: Access<'_, T, Self>, ...) -> SocketResult<...>
```

### 6. Run Tests

```bash
# Test p1 and p2
cargo test -p wash-wasi --features p1,p2

# Test all features
cargo test -p wash-wasi --features p1,p2,p3

# Run with specific test
cargo test -p wash-wasi --features p1,p2 -- p2_api_reactor
```

### 7. Format Code

```bash
cargo +nightly fmt
```

## Troubleshooting

### "failed to locate a WIT error type"

The bindgen trappable_error_type configuration may have changed. Check upstream `bindings.rs` for the correct format.

### Missing `assert_test_exists` errors

The `foreach_p1!`, `foreach_p2!`, `foreach_p3!`, or `foreach_p2_api!` macros require test functions to exist for each test component. Add stub functions for tests handled by other crates.

### Import resolution errors

Module paths may have been reorganized. Check the upstream crate structure.

### `func_new_async` signature changes

The closure signature for `func_new_async` may change between versions. Check the wasmtime documentation for the current signature.

### P3 socket tests failing with `todo!()` panics

The wash-specific loopback socket implementation is not fully implemented for P3. These tests will fail:
- `p3_sockets_tcp_bind`
- `p3_sockets_tcp_connect`
- `p3_sockets_tcp_sample_application`
- `p3_sockets_tcp_sockopts`
- `p3_sockets_tcp_states`
- `p3_sockets_tcp_streams`
- `p3_sockets_udp_*` tests

This is expected behavior. The P3 loopback methods in `TcpSocket` and `UdpSocket` enums return `todo!()` for the `Loopback` and `Unspecified` variants.

## Quick Reference: Finding Upstream Code

```bash
# Find the wasmtime checkout path
ls ~/.cargo/git/checkouts/wasmtime-*/

# The version-specific directory will be inside (e.g., 3dda916 for v41)
# Full path example:
# ~/.cargo/git/checkouts/wasmtime-ae46461068d65b15/3dda916/crates/wasi/

# Compare specific files
diff wash_file upstream_file

# Recursive diff of entire src directory
diff -ru ./crates/wasi/src ~/.cargo/git/checkouts/wasmtime-*/*/crates/wasi/src

# Recursive diff of test directory
diff -ru ./crates/wasi/tests ~/.cargo/git/checkouts/wasmtime-*/*/crates/wasi/tests
```

## Version History

| wash-wasi  | wasmtime | Notes                                                          |
|------------|----------|----------------------------------------------------------------|
| 2.0.0-rc.6 | v41.0.0  | WIT consolidation, Command API changes, Access type, TryFrom clocks |
| 2.0.0-rc.5 | v38.0.0  | Baseline before major p3 sync |

## Related Resources

- [Wasmtime Changelog](https://github.com/bytecodealliance/wasmtime/blob/main/CHANGELOG.md)
- [Wasmtime WASI Crate](https://github.com/bytecodealliance/wasmtime/tree/main/crates/wasi)
- [Component Model Docs](https://component-model.bytecodealliance.org/)

  The sync is correct. Here are the intentional differences from upstream:
  Change: TcpSocket renamed to NetworkTcpSocket
  Reason: Make room for wash-specific enum
  ────────────────────────────────────────
  Change: New TcpSocket enum with Network/Loopback/Unspecified variants
  Reason: Loopback support

  ────────────────────────────────────────
  Change: ConnectingTcpStream type instead of TcpStream for connecting
  Reason: Loopback needs different stream type
  ────────────────────────────────────────
  Change: start_bind, start_connect, finish_connect, start_listen take &mut
    loopback param
  Reason: Loopback routing
  ────────────────────────────────────────
  Change: P3 wrapper methods on TcpSocket enum
  Reason: Delegate to NetworkTcpSocket, todo!() for loopback
