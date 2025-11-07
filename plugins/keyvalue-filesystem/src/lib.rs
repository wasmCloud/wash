//! This component implements `wasi:keyvalue` in terms of `wasi:filesystem`, allowing
//! components to use the keyvalue API with a backing filesystem for development and testing.
//!
//! Storage layout:
//! - Each bucket is a directory under the root preopened directory
//! - Each key-value pair is stored as a file within the bucket directory
//! - The filename is the key, and the file contents are the value bytes

use wasi::filesystem::types::{Descriptor, DescriptorFlags, DescriptorType, OpenFlags, PathFlags};

use crate::bindings::exports::wasi::keyvalue::store::{self, Error, KeyResponse};
use crate::bindings::exports::wasi::keyvalue::{atomics, batch};

/// Generated WIT bindings
pub mod bindings;
/// `wasmcloud:wash/plugin` implementation
pub mod plugin;

/// A bucket backed by a filesystem directory
pub type BucketImpl = String;

struct Component;

impl store::Guest for Component {
    type Bucket = BucketImpl;

    fn open(identifier: String) -> Result<store::Bucket, Error> {
        let (root_dir, _) = get_root_dir().map_err(Error::Other)?;

        // Try to open the bucket directory, create it if it doesn't exist
        match root_dir.open_at(
            PathFlags::empty(),
            &identifier,
            OpenFlags::CREATE | OpenFlags::DIRECTORY,
            DescriptorFlags::READ | DescriptorFlags::MUTATE_DIRECTORY,
        ) {
            Ok(_) => (),
            Err(wasi::filesystem::types::ErrorCode::NoEntry) => {
                // Directory doesn't exist, create it
                root_dir
                    .create_directory_at(&identifier)
                    .map_err(fs_error_to_kv_error)?;
            }
            Err(e) => return Err(fs_error_to_kv_error(e)),
        }

        Ok(store::Bucket::new(identifier))
    }
}

impl atomics::Guest for Component {
    fn increment(bucket: store::BucketBorrow<'_>, key: String, delta: u64) -> Result<u64, Error> {
        let bucket_impl = bucket.get::<BucketImpl>();

        // Read current value
        let current_bytes = store::GuestBucket::get(bucket_impl, key.clone())?;

        // Parse as u64 (default to 0 if not present or invalid)
        let current_value = current_bytes
            .and_then(|bytes| {
                if bytes.len() == 8 {
                    Some(u64::from_le_bytes(bytes.try_into().ok()?))
                } else {
                    None
                }
            })
            .unwrap_or(0);

        // Calculate new value
        let new_value = current_value.wrapping_add(delta);

        // Write back
        store::GuestBucket::set(bucket_impl, key, new_value.to_le_bytes().to_vec())?;

        Ok(new_value)
    }
}

/// Get the root directory of the filesystem. The implementation of `wasi:keyvalue` for wash
/// assumes that the root directory is the last preopened directory provided by the WASI runtime.
///
/// This is the only function that interacts with wasi:filesystem directly as it's required
/// to get the root directory for all operations.
fn get_root_dir() -> Result<(Descriptor, String), String> {
    let mut dirs = ::wasi::filesystem::preopens::get_directories();
    if dirs.len() > 1 {
        eprintln!("multiple preopened directories found, using the last one provided");
    }

    if let Some(dir) = dirs.pop() {
        Ok((dir.0, dir.1))
    } else {
        Err("No preopened directories found".to_string())
    }
}

/// Convert a filesystem error to a keyvalue error
fn fs_error_to_kv_error(e: wasi::filesystem::types::ErrorCode) -> Error {
    match e {
        wasi::filesystem::types::ErrorCode::Access => Error::AccessDenied,
        wasi::filesystem::types::ErrorCode::NotDirectory
        | wasi::filesystem::types::ErrorCode::NoEntry => Error::NoSuchStore,
        _ => Error::Other(e.to_string()),
    }
}

// ============================================================================
// Store Bucket Resource Implementation
// ============================================================================

impl store::GuestBucket for BucketImpl {
    fn get(&self, key: String) -> Result<Option<Vec<u8>>, Error> {
        let (root_dir, _) = get_root_dir().map_err(Error::Other)?;

        // Open the bucket directory
        let bucket_dir = root_dir
            .open_at(
                PathFlags::empty(),
                self,
                OpenFlags::DIRECTORY,
                DescriptorFlags::READ | DescriptorFlags::MUTATE_DIRECTORY,
            )
            .map_err(fs_error_to_kv_error)?;

        // Try to open the key file
        let file = match bucket_dir.open_at(
            PathFlags::empty(),
            &key,
            OpenFlags::empty(),
            DescriptorFlags::READ,
        ) {
            Ok(f) => f,
            Err(wasi::filesystem::types::ErrorCode::NoEntry) => return Ok(None),
            Err(e) => return Err(fs_error_to_kv_error(e)),
        };

        // Check if it's a regular file
        let stat = file.stat().map_err(fs_error_to_kv_error)?;
        if stat.type_ != DescriptorType::RegularFile {
            return Ok(None);
        }

        // Read the file contents
        let stream = file
            .read_via_stream(0)
            .map_err(|e| Error::Other(e.to_string()))?;

        let mut buffer = Vec::new();
        let chunk_size = 4096;

        loop {
            match stream.read(chunk_size) {
                Ok(data) if data.is_empty() => break,
                Ok(data) => buffer.extend_from_slice(&data),
                Err(wasi::io::streams::StreamError::Closed) => break,
                Err(e) => return Err(Error::Other(format!("read error: {:?}", e))),
            }
        }

        Ok(Some(buffer))
    }

    fn set(&self, key: String, value: Vec<u8>) -> Result<(), Error> {
        let (root_dir, _) = get_root_dir().map_err(Error::Other)?;

        // Open the bucket directory
        let bucket_dir = root_dir
            .open_at(
                PathFlags::empty(),
                self,
                OpenFlags::DIRECTORY,
                DescriptorFlags::READ | DescriptorFlags::MUTATE_DIRECTORY,
            )
            .map_err(fs_error_to_kv_error)?;

        // Create or truncate the key file
        let file = bucket_dir
            .open_at(
                PathFlags::empty(),
                &key,
                OpenFlags::CREATE | OpenFlags::TRUNCATE,
                DescriptorFlags::WRITE,
            )
            .map_err(fs_error_to_kv_error)?;

        // Write the value
        let stream = file
            .write_via_stream(0)
            .map_err(|e| Error::Other(e.to_string()))?;

        let mut offset = 0;
        let chunk_size = 4096;
        while offset < value.len() {
            let end = std::cmp::min(offset + chunk_size, value.len());
            let chunk = &value[offset..end];

            match stream.write(chunk) {
                Ok(_) => offset += chunk.len(),
                Err(e) => return Err(Error::Other(format!("write error: {:?}", e))),
            }
        }

        // Flush the stream
        stream.flush().map_err(|e| Error::Other(e.to_string()))?;

        Ok(())
    }

    fn delete(&self, key: String) -> Result<(), Error> {
        let (root_dir, _) = get_root_dir().map_err(Error::Other)?;

        // Open the bucket directory
        let bucket_dir = root_dir
            .open_at(
                PathFlags::empty(),
                self,
                OpenFlags::DIRECTORY,
                DescriptorFlags::READ | DescriptorFlags::MUTATE_DIRECTORY,
            )
            .map_err(fs_error_to_kv_error)?;

        // Try to delete the file (ignore if it doesn't exist)
        match bucket_dir.unlink_file_at(&key) {
            Ok(_) => Ok(()),
            Err(wasi::filesystem::types::ErrorCode::NoEntry) => Ok(()), // Succeed if already deleted
            Err(e) => Err(fs_error_to_kv_error(e)),
        }
    }

    fn exists(&self, key: String) -> Result<bool, Error> {
        let (root_dir, _) = get_root_dir().map_err(Error::Other)?;

        // Open the bucket directory
        let bucket_dir = root_dir
            .open_at(
                PathFlags::empty(),
                self,
                OpenFlags::DIRECTORY,
                DescriptorFlags::READ,
            )
            .map_err(fs_error_to_kv_error)?;

        // Try to stat the file
        match bucket_dir.stat_at(PathFlags::empty(), &key) {
            Ok(stat) => Ok(stat.type_ == DescriptorType::RegularFile),
            Err(wasi::filesystem::types::ErrorCode::NoEntry) => Ok(false),
            Err(e) => Err(fs_error_to_kv_error(e)),
        }
    }

    fn list_keys(&self, cursor: Option<u64>) -> Result<KeyResponse, Error> {
        let (root_dir, _) = get_root_dir().map_err(Error::Other)?;

        // Open the bucket directory
        let bucket_dir = root_dir
            .open_at(
                PathFlags::empty(),
                self,
                OpenFlags::DIRECTORY,
                DescriptorFlags::READ,
            )
            .map_err(fs_error_to_kv_error)?;

        // Read directory entries
        let entries = bucket_dir
            .read_directory()
            .map_err(|e| Error::Other(e.to_string()))?;

        let mut keys = Vec::new();
        let mut entry_count: u64 = 0;
        let start_from = cursor.unwrap_or(0);

        loop {
            match entries.read_directory_entry() {
                Ok(Some(entry)) => {
                    // Skip special directories and metadata directories
                    if entry.name == "." || entry.name == ".." || entry.name.starts_with('.') {
                        continue;
                    }

                    // Only include regular files
                    if entry.type_ == DescriptorType::RegularFile {
                        // Handle cursor-based pagination (skip entries before cursor)
                        if entry_count < start_from {
                            entry_count += 1;
                            continue;
                        }

                        keys.push(entry.name);
                        entry_count += 1;

                        // Limit to 1000 keys per page for performance
                        if keys.len() >= 1000 {
                            break;
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => return Err(Error::Other(e.to_string())),
            }
        }

        // Determine next cursor (position of next entry to read)
        let next_cursor = if keys.len() >= 1000 {
            Some(entry_count)
        } else {
            None
        };

        Ok(KeyResponse {
            keys,
            cursor: next_cursor,
        })
    }
}

// ============================================================================
// Batch Interface Implementation
// ============================================================================

impl batch::Guest for Component {
    fn get_many(
        bucket: store::BucketBorrow<'_>,
        keys: Vec<String>,
    ) -> Result<Vec<Option<(String, Vec<u8>)>>, Error> {
        let bucket_impl = bucket.get::<BucketImpl>();
        let mut results = Vec::with_capacity(keys.len());

        for key in keys {
            match store::GuestBucket::get(bucket_impl, key.clone()) {
                Ok(Some(value)) => results.push(Some((key, value))),
                Ok(None) => results.push(None),
                Err(e) => return Err(e),
            }
        }

        Ok(results)
    }

    fn set_many(
        bucket: store::BucketBorrow<'_>,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> Result<(), Error> {
        let bucket_impl = bucket.get::<BucketImpl>();
        for (key, value) in key_values {
            store::GuestBucket::set(bucket_impl, key, value)?;
        }
        Ok(())
    }

    fn delete_many(bucket: store::BucketBorrow<'_>, keys: Vec<String>) -> Result<(), Error> {
        let bucket_impl = bucket.get::<BucketImpl>();
        for key in keys {
            store::GuestBucket::delete(bucket_impl, key)?;
        }
        Ok(())
    }
}

// Export the component implementation via the generated bindings
bindings::export!(Component with_types_in bindings);
