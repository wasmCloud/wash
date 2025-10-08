//! # WASI KeyValue Memory Plugin
//!
//! This module implements an in-memory keyvalue plugin for the wasmCloud runtime,
//! providing the `wasi:keyvalue@0.2.0-draft` interfaces for development and testing scenarios.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

const WASI_KEYVALUE_ID: &str = "wasi-keyvalue";
use tokio::sync::RwLock;
use wasmtime::component::Resource;

use crate::{
    engine::{
        ctx::Ctx,
        workload::{ResolvedWorkload, WorkloadComponent},
    },
    plugin::HostPlugin,
    wit::{WitInterface, WitWorld},
};

mod bindings {
    wasmtime::component::bindgen!({
        world: "keyvalue",
        trappable_imports: true,
        async: true,
        with: {
            "wasi:keyvalue/store/bucket": crate::plugin::wasi_keyvalue::BucketHandle,
        },
    });
}

use bindings::wasi::keyvalue::store::{Error as StoreError, KeyResponse};

/// In-memory bucket representation
#[derive(Clone, Debug)]
pub struct BucketData {
    pub name: String,
    pub data: HashMap<String, Vec<u8>>,
    pub created_at: u64,
}

/// Resource representation for a bucket (key-value store)
pub type BucketHandle = String;

/// Memory-based keyvalue plugin
#[derive(Clone, Default)]
pub struct WasiKeyvalue {
    /// Storage for all buckets, keyed by workload ID, then bucket name
    storage: Arc<RwLock<HashMap<String, HashMap<String, BucketData>>>>,
}

impl WasiKeyvalue {
    pub fn new() -> Self {
        Self {
            storage: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn get_timestamp() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

// Implementation for the store interface
impl bindings::wasi::keyvalue::store::Host for Ctx {
    async fn open(
        &mut self,
        identifier: String,
    ) -> anyhow::Result<Result<Resource<BucketHandle>, StoreError>> {
        let Some(plugin) = self.get_plugin::<WasiKeyvalue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.id.clone()).or_default();

        // Create bucket if it doesn't exist
        if !workload_storage.contains_key(&identifier) {
            let bucket_data = BucketData {
                name: identifier.clone(),
                data: HashMap::new(),
                created_at: WasiKeyvalue::get_timestamp(),
            };
            workload_storage.insert(identifier.clone(), bucket_data);
        }

        let resource = self.table.push(identifier)?;
        Ok(Ok(resource))
    }
}

// Resource host trait implementations for bucket
impl bindings::wasi::keyvalue::store::HostBucket for Ctx {
    async fn get(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> anyhow::Result<Result<Option<Vec<u8>>, StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<WasiKeyvalue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let storage = plugin.storage.read().await;
        let empty_map = HashMap::new();
        let workload_storage = storage.get(&self.id).unwrap_or(&empty_map);

        match workload_storage.get(bucket_name) {
            Some(bucket_data) => {
                let value = bucket_data.data.get(&key).cloned();
                Ok(Ok(value))
            }
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }

    async fn set(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
        value: Vec<u8>,
    ) -> anyhow::Result<Result<(), StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<WasiKeyvalue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.id.clone()).or_default();

        match workload_storage.get_mut(bucket_name) {
            Some(bucket_data) => {
                bucket_data.data.insert(key, value);
                Ok(Ok(()))
            }
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }

    async fn delete(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> anyhow::Result<Result<(), StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<WasiKeyvalue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.id.clone()).or_default();

        match workload_storage.get_mut(bucket_name) {
            Some(bucket_data) => {
                bucket_data.data.remove(&key);
                Ok(Ok(()))
            }
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }

    async fn exists(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
    ) -> anyhow::Result<Result<bool, StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<WasiKeyvalue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let storage = plugin.storage.read().await;
        let empty_map = HashMap::new();
        let workload_storage = storage.get(&self.id).unwrap_or(&empty_map);

        match workload_storage.get(bucket_name) {
            Some(bucket_data) => Ok(Ok(bucket_data.data.contains_key(&key))),
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }

    async fn list_keys(
        &mut self,
        bucket: Resource<BucketHandle>,
        cursor: Option<u64>,
    ) -> anyhow::Result<Result<KeyResponse, StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<WasiKeyvalue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let storage = plugin.storage.read().await;
        let empty_map = HashMap::new();
        let workload_storage = storage.get(&self.id).unwrap_or(&empty_map);

        match workload_storage.get(bucket_name) {
            Some(bucket_data) => {
                let mut keys: Vec<String> = bucket_data.data.keys().cloned().collect();
                keys.sort(); // Ensure consistent ordering

                // Simple cursor-based pagination - cursor is the index from previous page
                let start_index = cursor.unwrap_or(0) as usize;

                // Return up to 100 keys per page
                const PAGE_SIZE: usize = 100;
                let end_index = std::cmp::min(start_index + PAGE_SIZE, keys.len());
                let page_keys = keys[start_index..end_index].to_vec();

                // Set next cursor if there are more keys
                let next_cursor = if end_index < keys.len() {
                    Some(end_index as u64)
                } else {
                    None
                };

                Ok(Ok(KeyResponse {
                    keys: page_keys,
                    cursor: next_cursor,
                }))
            }
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }

    async fn drop(&mut self, rep: Resource<BucketHandle>) -> anyhow::Result<()> {
        tracing::debug!(
            workload_id = self.id,
            resource_id = ?rep,
            "Dropping bucket resource"
        );
        self.table.delete(rep)?;
        Ok(())
    }
}

// Implementation for the atomics interface
impl bindings::wasi::keyvalue::atomics::Host for Ctx {
    async fn increment(
        &mut self,
        bucket: Resource<BucketHandle>,
        key: String,
        delta: u64,
    ) -> anyhow::Result<Result<u64, StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<WasiKeyvalue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.id.clone()).or_default();

        match workload_storage.get_mut(bucket_name) {
            Some(bucket_data) => {
                // Get current value, treating missing key as 0
                let current_bytes = bucket_data.data.get(&key);
                let current_value = if let Some(bytes) = current_bytes {
                    // Try to parse as u64 from 8-byte array
                    if bytes.len() == 8 {
                        u64::from_le_bytes(bytes.clone().try_into().unwrap_or([0; 8]))
                    } else {
                        // Try to parse as string representation
                        String::from_utf8_lossy(bytes).parse::<u64>().unwrap_or(0)
                    }
                } else {
                    0
                };

                let new_value = current_value.saturating_add(delta);

                // Store as 8-byte little-endian representation
                bucket_data
                    .data
                    .insert(key, new_value.to_le_bytes().to_vec());

                Ok(Ok(new_value))
            }
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }
}

// Implementation for the batch interface
impl bindings::wasi::keyvalue::batch::Host for Ctx {
    async fn get_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        keys: Vec<String>,
    ) -> anyhow::Result<Result<Vec<Option<(String, Vec<u8>)>>, StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<WasiKeyvalue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let storage = plugin.storage.read().await;
        let empty_map = HashMap::new();
        let workload_storage = storage.get(&self.id).unwrap_or(&empty_map);

        match workload_storage.get(bucket_name) {
            Some(bucket_data) => {
                let results: Vec<Option<(String, Vec<u8>)>> = keys
                    .into_iter()
                    .map(|key| {
                        bucket_data
                            .data
                            .get(&key)
                            .cloned()
                            .map(|value| (key, value))
                    })
                    .collect();
                Ok(Ok(results))
            }
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }

    async fn set_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        key_values: Vec<(String, Vec<u8>)>,
    ) -> anyhow::Result<Result<(), StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<WasiKeyvalue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.id.clone()).or_default();

        match workload_storage.get_mut(bucket_name) {
            Some(bucket_data) => {
                for (key, value) in key_values {
                    bucket_data.data.insert(key, value);
                }
                Ok(Ok(()))
            }
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }

    async fn delete_many(
        &mut self,
        bucket: Resource<BucketHandle>,
        keys: Vec<String>,
    ) -> anyhow::Result<Result<(), StoreError>> {
        let bucket_name = self.table.get(&bucket)?;

        let Some(plugin) = self.get_plugin::<WasiKeyvalue>(WASI_KEYVALUE_ID) else {
            return Ok(Err(StoreError::Other(
                "keyvalue plugin not available".to_string(),
            )));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.id.clone()).or_default();

        match workload_storage.get_mut(bucket_name) {
            Some(bucket_data) => {
                for key in keys {
                    bucket_data.data.remove(&key);
                }
                Ok(Ok(()))
            }
            None => Ok(Err(StoreError::Other(format!(
                "bucket '{bucket_name}' does not exist"
            )))),
        }
    }
}

#[async_trait::async_trait]
impl HostPlugin for WasiKeyvalue {
    fn id(&self) -> &'static str {
        WASI_KEYVALUE_ID
    }

    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasi:keyvalue/store,atomics,batch@0.2.0-draft",
            )]),
            ..Default::default()
        }
    }

    async fn on_component_bind(
        &self,
        component: &mut WorkloadComponent,
        interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        // Check if any of the interfaces are wasi:keyvalue related
        let has_keyvalue = interfaces
            .iter()
            .any(|i| i.namespace == "wasi" && i.package == "keyvalue");

        if !has_keyvalue {
            tracing::warn!(
                "WasiKeyvalue plugin requested for non-wasi:keyvalue interface(s): {:?}",
                interfaces
            );
            return Ok(());
        }

        tracing::debug!(
            workload_id = component.id(),
            "Adding keyvalue interfaces to linker for workload"
        );
        let linker = component.linker();

        bindings::wasi::keyvalue::store::add_to_linker(linker, |ctx| ctx)?;
        bindings::wasi::keyvalue::atomics::add_to_linker(linker, |ctx| ctx)?;
        bindings::wasi::keyvalue::batch::add_to_linker(linker, |ctx| ctx)?;

        let id = component.id();
        tracing::debug!(
            workload_id = id,
            "Successfully added keyvalue interfaces to linker for workload"
        );

        // Initialize storage for this workload
        let mut storage = self.storage.write().await;
        storage.insert(id.to_string(), HashMap::new());

        tracing::debug!("WasiKeyvalue plugin bound to workload '{id}'");

        Ok(())
    }

    async fn on_workload_unbind(
        &self,
        workload_handle: &ResolvedWorkload,
        _interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        let id = workload_handle.id();
        // Clean up storage for this workload
        let mut storage = self.storage.write().await;
        storage.remove(id);

        tracing::debug!("WasiKeyvalue plugin unbound from workload '{id}'");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasi_keyvalue_creation() {
        let keyvalue = WasiKeyvalue::new();
        assert!(keyvalue.storage.try_read().is_ok());
    }

    #[test]
    fn test_get_timestamp() {
        let timestamp = WasiKeyvalue::get_timestamp();
        assert!(timestamp > 0);
    }

    #[test]
    fn test_bucket_data_creation() {
        let bucket = BucketData {
            name: "test-bucket".to_string(),
            data: HashMap::new(),
            created_at: WasiKeyvalue::get_timestamp(),
        };

        assert_eq!(bucket.name, "test-bucket");
        assert!(bucket.data.is_empty());
        assert!(bucket.created_at > 0);
    }

    #[tokio::test]
    async fn test_storage_operations() {
        let keyvalue = WasiKeyvalue::new();

        // Test write access
        {
            let mut storage = keyvalue.storage.write().await;
            storage.insert("workload1".to_string(), HashMap::new());
        }

        // Test read access
        {
            let storage = keyvalue.storage.read().await;
            assert!(storage.contains_key("workload1"));
        }
    }

    #[test]
    fn test_batch_operations_data_structures() {
        // Test that we can create the data structures for batch operations
        let key_values = [
            ("key1".to_string(), b"value1".to_vec()),
            ("key2".to_string(), b"value2".to_vec()),
        ];
        assert_eq!(key_values.len(), 2);

        let keys = ["key1".to_string(), "key2".to_string()];
        assert_eq!(keys.len(), 2);

        let results: Vec<Option<(String, Vec<u8>)>> = vec![
            Some(("key1".to_string(), b"value1".to_vec())),
            None, // key not found
        ];
        assert_eq!(results.len(), 2);
        assert!(results[0].is_some());
        assert!(results[1].is_none());
    }
}
