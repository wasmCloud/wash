# Keyvalue Filesystem

This component is a wash plugin that exports `wasi:keyvalue/store@0.2.0-draft`, `wasi:keyvalue/atomics@0.2.0-draft`, and `wasi:keyvalue/batch@0.2.0-draft` and implements the keyvalue functions in terms of `wasi:filesystem` APIs. This is a reference implementation for how you can use a component plugin to extend the functionality of `wash dev`.

## Features

- **Store Operations**: Basic key-value operations (get, set, delete, exists, list-keys)
- **Batch Operations**: Efficient multi-key operations (get-many, set-many, delete-many)
- **Atomic Operations**: Atomic increment operations
- **Editable Storage**: Values are stored as files on the filesystem, allowing direct editing

## Requirements

This component must be passed a directory as a `wasi:filesystem/preopen` with read and write permissions. This directory will be used as the root for all keyvalue operations.

## Storage Layout

The plugin organizes data as follows:

```
root_directory/
├── bucket1/
│   ├── key1           (file containing value bytes)
│   ├── key2
│   └── key3
├── bucket2/
│   ├── key_a
│   └── key_b
```

- Each bucket is a subdirectory under the preopened root directory
- Each key-value pair is stored as a file where:
  - The filename is the key
  - The file contents are the value bytes
- You can directly edit these files to modify values

## Building

To build the plugin:

```bash
cargo build --target wasm32-wasip2 --release
```

The compiled plugin will be at:
```
target/wasm32-wasip2/release/keyvalue_filesystem.wasm
```

## Usage with wash dev

### Automatic Registration

The plugin automatically registers with `wash dev` via the `DevRegister` hook. When a component imports `wasi:keyvalue/*`, wash will automatically link it to this plugin if available.

### Installing the Plugin

To make the plugin available to wash:

1. Build the plugin (see above)
2. Install it with wash:
   ```bash
   wash plugin install target/wasm32-wasip2/release/keyvalue_filesystem.wasm
   ```

### Using with Components

When running `wash dev`, components that import `wasi:keyvalue` interfaces will automatically be linked to this plugin:

```wit
// In your component's WIT file
package my:component;

world my-component {
    import wasi:keyvalue/store@0.2.0-draft2;
    import wasi:keyvalue/atomics@0.2.0-draft2;
    import wasi:keyvalue/batch@0.2.0-draft2;
    // ... rest of your component
}
```

### Configuring Storage Location

The plugin requires a preopened directory. With `wash dev`, you can configure this in your `wasmcloud.toml`:

```toml
[[component]]
name = "my-component"
path = "./path/to/component.wasm"

[component.config]
# Configure volume mounts for the keyvalue plugin
volumes = [
    { name = "keyvalue-data", path = "/data", read_only = false }
]
```

### Creating Buckets

Before using the keyvalue operations, you need to create bucket directories:

```bash
# Create a bucket directory
mkdir -p /data/my-bucket
```

Then in your component code:

```rust
use wasi::keyvalue::store;

// Open the bucket
let bucket = store::open("my-bucket")?;

// Set a value
bucket.set("my-key", b"my-value".to_vec())?;

// Get a value
let value = bucket.get("my-key")?;
```

### Editing Values Directly

Since values are stored as files, you can edit them directly:

```bash
# View a value
cat /data/my-bucket/my-key

# Edit a value
echo "new-value" > /data/my-bucket/my-key

# List all keys in a bucket
ls /data/my-bucket/
```

This is especially useful for:
- Debugging and development
- Manual data inspection
- Quick prototyping
- Testing different scenarios

## Examples

### Basic Store Operations

```rust
use wasi::keyvalue::store;

// Open a bucket
let bucket = store::open("my-bucket")?;

// Set a value
bucket.set("user:123", b"John Doe".to_vec())?;

// Get a value
if let Some(value) = bucket.get("user:123")? {
    let name = String::from_utf8(value)?;
    println!("User: {}", name);
}

// Check if key exists
if bucket.exists("user:123")? {
    println!("User exists!");
}

// Delete a key
bucket.delete("user:123")?;

// List all keys
let response = bucket.list_keys(None)?;
for key in response.keys {
    println!("Key: {}", key);
}
```

### Batch Operations

```rust
use wasi::keyvalue::batch;

let bucket = store::open("my-bucket")?;

// Set multiple values at once
batch::set_many(&bucket, vec![
    ("user:1".to_string(), b"Alice".to_vec()),
    ("user:2".to_string(), b"Bob".to_vec()),
    ("user:3".to_string(), b"Charlie".to_vec()),
])?;

// Get multiple values
let results = batch::get_many(&bucket, vec![
    "user:1".to_string(),
    "user:2".to_string(),
    "user:999".to_string(), // Non-existent key
])?;

for result in results {
    if let Some((key, value)) = result {
        println!("{}: {:?}", key, value);
    }
}

// Delete multiple keys
batch::delete_many(&bucket, vec![
    "user:1".to_string(),
    "user:2".to_string(),
])?;
```

### Atomic Operations

```rust
use wasi::keyvalue::atomics;

let bucket = store::open("counters")?;

// Atomic increment (perfect for counters)
let new_count = atomics::increment(&bucket, "page_views".to_string(), 1)?;
println!("Page views: {}", new_count);

// Increment by larger delta
let total = atomics::increment(&bucket, "total_requests".to_string(), 100)?;
println!("Total requests: {}", total);
```

## Implementation Notes

- **Atomic increment**: Stores u64 values as 8-byte little-endian encoded files
- **Pagination**: list-keys returns up to 1000 keys per page with u64 cursor support
- **Error handling**: Follows WASI keyvalue error semantics (NoSuchStore, AccessDenied, Other)
