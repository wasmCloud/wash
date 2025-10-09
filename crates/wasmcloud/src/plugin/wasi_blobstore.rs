//! # WASI Blobstore Memory Plugin
//!
//! This module implements an in-memory blobstore plugin for the wasmCloud runtime,
//! providing the `wasi:blobstore@0.2.0-draft` interface for development and testing scenarios.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::SystemTime,
};

const WASI_BLOBSTORE_ID: &str = "wasi-blobstore";
use tokio::sync::RwLock;
use wasmtime::component::Resource;
use wasmtime_wasi::{
    InputStream, OutputStream,
    pipe::{MemoryInputPipe, MemoryOutputPipe},
};

use crate::{
    engine::ctx::Ctx,
    engine::workload::{ResolvedWorkload, WorkloadComponent},
    plugin::HostPlugin,
    wit::{WitInterface, WitWorld},
};

mod bindings {
    wasmtime::component::bindgen!({
        world: "blobstore",
        trappable_imports: true,
        async: true,
        with: {
            "wasi:io": ::wasmtime_wasi::bindings::io,
            "wasi:blobstore/container/container": String,
            "wasi:blobstore/container/stream-object-names": crate::plugin::wasi_blobstore::StreamObjectNamesHandle,
            "wasi:blobstore/types/incoming-value": crate::plugin::wasi_blobstore::IncomingValueHandle,
            "wasi:blobstore/types/outgoing-value": crate::plugin::wasi_blobstore::OutgoingValueHandle,
        },
    });
}

use bindings::wasi::blobstore::{
    container::Error as ContainerError,
    types::{
        ContainerMetadata, ContainerName, Error as BlobstoreError, ObjectId, ObjectMetadata,
        ObjectName,
    },
};

/// Metadata for an object stored in memory
#[derive(Clone, Debug)]
pub struct ObjectData {
    pub name: String,
    pub container: String,
    pub data: Vec<u8>,
    pub created_at: u64,
}

/// In-memory container representation
#[derive(Clone, Debug)]
pub struct ContainerData {
    pub name: String,
    pub created_at: u64,
    pub objects: HashMap<String, ObjectData>,
}

/// Resource representation for an incoming value (data being read)
pub type IncomingValueHandle = Vec<u8>;

/// Resource representation for an outgoing value (data being written)
pub struct OutgoingValueHandle {
    pub pipe: MemoryOutputPipe,
    pub container_name: Option<String>,
    pub object_name: Option<String>,
}

/// Resource representation for streaming object names
#[derive(Debug)]
pub struct StreamObjectNamesHandle {
    pub container_name: String,
    pub workload_id: String,
    pub objects: Vec<String>,
    pub position: usize,
}

/// Memory-based blobstore plugin
#[derive(Clone, Default)]
pub struct WasiBlobstore {
    /// Storage for all containers, keyed by workload ID
    storage: Arc<RwLock<HashMap<String, HashMap<String, ContainerData>>>>,
    /// The maximum size for objects stored in the blobstore
    max_object_size: usize,
}

impl WasiBlobstore {
    pub fn new(max_object_size: Option<usize>) -> Self {
        Self {
            storage: Arc::new(RwLock::new(HashMap::new())),
            max_object_size: max_object_size.unwrap_or(1_000_000), // 1mb limit by default
        }
    }

    fn get_timestamp() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

// Implementation for the main blobstore interface
impl bindings::wasi::blobstore::blobstore::Host for Ctx {
    async fn create_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<String>, BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<WasiBlobstore>(WASI_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.id.clone()).or_default();

        if workload_storage.contains_key(&name) {
            return Ok(Err(format!("container '{name}' already exists")));
        }

        let container_data = ContainerData {
            name: name.clone(),
            created_at: WasiBlobstore::get_timestamp(),
            objects: HashMap::new(),
        };

        workload_storage.insert(name.clone(), container_data);
        let resource = self.table.push(name)?;
        Ok(Ok(resource))
    }

    async fn get_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<Resource<String>, BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<WasiBlobstore>(WASI_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let storage = plugin.storage.read().await;
        let empty_map = HashMap::new();
        let workload_storage = storage.get(&self.id).unwrap_or(&empty_map);

        if !workload_storage.contains_key(&name) {
            return Ok(Err(format!("container '{name}' does not exist")));
        }

        let resource = self.table.push(name)?;
        Ok(Ok(resource))
    }

    async fn delete_container(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<(), BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<WasiBlobstore>(WASI_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.id.clone()).or_default();

        workload_storage.remove(&name);
        Ok(Ok(()))
    }

    async fn container_exists(
        &mut self,
        name: ContainerName,
    ) -> anyhow::Result<Result<bool, BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<WasiBlobstore>(WASI_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let storage = plugin.storage.read().await;
        let empty_map = HashMap::new();
        let workload_storage = storage.get(&self.id).unwrap_or(&empty_map);

        Ok(Ok(workload_storage.contains_key(&name)))
    }

    async fn copy_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), BlobstoreError>> {
        let Some(plugin) = self.get_plugin::<WasiBlobstore>(WASI_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.id.clone()).or_default();

        // Get source object data (clone to avoid borrow conflicts)
        let src_object_data = {
            let src_container = match workload_storage.get(&src.container) {
                Some(container) => container,
                None => {
                    return Ok(Err(format!(
                        "source container '{}' does not exist",
                        src.container
                    )));
                }
            };

            match src_container.objects.get(&src.object) {
                Some(object) => object.clone(),
                None => {
                    return Ok(Err(format!(
                        "source object '{}' does not exist",
                        src.object
                    )));
                }
            }
        };

        // Ensure destination container exists and copy object
        let dest_container = match workload_storage.get_mut(&dest.container) {
            Some(container) => container,
            None => {
                return Ok(Err(format!(
                    "destination container '{}' does not exist",
                    dest.container
                )));
            }
        };

        let mut copied_object = src_object_data;
        copied_object.name = dest.object.clone();
        copied_object.container = dest.container.clone();
        copied_object.created_at = WasiBlobstore::get_timestamp();

        dest_container.objects.insert(dest.object, copied_object);
        Ok(Ok(()))
    }

    async fn move_object(
        &mut self,
        src: ObjectId,
        dest: ObjectId,
    ) -> anyhow::Result<Result<(), BlobstoreError>> {
        // First copy the object
        let _ = self.copy_object(src.clone(), dest).await?;

        let Some(plugin) = self.get_plugin::<WasiBlobstore>(WASI_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        // Then delete the source
        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.id.clone()).or_default();

        if let Some(src_container) = workload_storage.get_mut(&src.container) {
            src_container.objects.remove(&src.object);
        }

        Ok(Ok(()))
    }
}

// Resource host trait implementations - these handle the lifecycle of each resource type
impl bindings::wasi::blobstore::container::HostContainer for Ctx {
    async fn name(
        &mut self,
        container: Resource<String>,
    ) -> anyhow::Result<Result<String, ContainerError>> {
        let container_name = self.table.get(&container)?;
        Ok(Ok(container_name.clone()))
    }

    async fn info(
        &mut self,
        container: Resource<String>,
    ) -> anyhow::Result<Result<ContainerMetadata, ContainerError>> {
        let container_name = self.table.get(&container)?;

        let Some(plugin) = self.get_plugin::<WasiBlobstore>(WASI_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let storage = plugin.storage.read().await;
        let empty_map = HashMap::new();
        let workload_storage = storage.get(&self.id).unwrap_or(&empty_map);

        match workload_storage.get(container_name) {
            Some(container_data) => Ok(Ok(ContainerMetadata {
                name: container_data.name.clone(),
                created_at: container_data.created_at,
            })),
            None => Ok(Err(format!("container '{container_name}' does not exist"))),
        }
    }

    async fn get_data(
        &mut self,
        container: Resource<String>,
        name: ObjectName,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Result<Resource<IncomingValueHandle>, ContainerError>> {
        let container_name = self.table.get(&container)?;

        tracing::debug!(
            container = container_name,
            object = name,
            start = start,
            end = end,
            workload_id = self.id,
            "Getting object data from container"
        );

        let Some(plugin) = self.get_plugin::<WasiBlobstore>(WASI_BLOBSTORE_ID) else {
            tracing::error!("blobstore plugin not available for get_data");
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let storage = plugin.storage.read().await;
        let empty_map = HashMap::new();
        let workload_storage = storage.get(&self.id).unwrap_or(&empty_map);

        match workload_storage.get(container_name) {
            Some(container_data) => match container_data.objects.get(&name) {
                Some(object_data) => {
                    let start_idx = start.min(object_data.data.len() as u64) as usize;
                    let end_idx = end.min(object_data.data.len() as u64) as usize;
                    let data_slice = object_data.data[start_idx..end_idx].to_vec();

                    tracing::debug!(
                        container = container_name,
                        object = name,
                        original_size = object_data.data.len(),
                        slice_size = data_slice.len(),
                        start_idx = start_idx,
                        end_idx = end_idx,
                        "Retrieved object data slice"
                    );

                    let resource = self.table.push(data_slice)?;
                    Ok(Ok(resource))
                }
                None => {
                    tracing::warn!(
                        container = container_name,
                        object = name,
                        "Object does not exist in container"
                    );
                    Ok(Err(format!("object '{name}' does not exist")))
                }
            },
            None => {
                tracing::warn!(
                    container = container_name,
                    workload_id = self.id,
                    "Container does not exist for workload"
                );
                Ok(Err(format!("container '{container_name}' does not exist")))
            }
        }
    }

    async fn write_data(
        &mut self,
        container: Resource<String>,
        name: ObjectName,
        data: Resource<OutgoingValueHandle>,
    ) -> anyhow::Result<Result<(), ContainerError>> {
        let container_name = self.table.get(&container)?.clone();

        tracing::debug!(
            container = container_name,
            object = name,
            workload_id = self.id,
            "Initiating write_data for object"
        );

        let Some(plugin) = self.get_plugin::<WasiBlobstore>(WASI_BLOBSTORE_ID) else {
            tracing::error!("blobstore plugin not available for write_data");
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        // Verify the container exists
        let storage = plugin.storage.read().await;
        let empty_map = HashMap::new();
        let workload_storage = storage.get(&self.id).unwrap_or(&empty_map);

        if !workload_storage.contains_key(&container_name) {
            tracing::warn!(
                container = container_name,
                workload_id = self.id,
                "Container does not exist for write_data"
            );
            return Ok(Err(format!("container '{container_name}' does not exist")));
        }
        drop(storage);

        // Store the container and object names - actual writing happens in finish()
        let outgoing_handle = self.table.get_mut(&data)?;
        outgoing_handle.container_name = Some(container_name.clone());
        outgoing_handle.object_name = Some(name.clone());

        tracing::debug!(
            container = container_name,
            object = name,
            "write_data setup complete, actual write will happen in finish()"
        );

        Ok(Ok(()))
    }

    async fn list_objects(
        &mut self,
        container: Resource<String>,
    ) -> anyhow::Result<Result<Resource<StreamObjectNamesHandle>, ContainerError>> {
        let container_name = self.table.get(&container)?;

        let Some(plugin) = self.get_plugin::<WasiBlobstore>(WASI_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let storage = plugin.storage.read().await;
        let empty_map = HashMap::new();
        let workload_storage = storage.get(&self.id).unwrap_or(&empty_map);

        match workload_storage.get(container_name) {
            Some(container_data) => {
                let objects: Vec<String> = container_data.objects.keys().cloned().collect();
                let handle = StreamObjectNamesHandle {
                    container_name: container_name.clone(),
                    workload_id: self.id.clone(),
                    objects,
                    position: 0,
                };
                let resource = self.table.push(handle)?;
                Ok(Ok(resource))
            }
            None => Ok(Err(format!("container '{container_name}' does not exist"))),
        }
    }

    async fn delete_object(
        &mut self,
        container: Resource<String>,
        name: ObjectName,
    ) -> anyhow::Result<Result<(), ContainerError>> {
        let container_name = self.table.get(&container)?;

        let Some(plugin) = self.get_plugin::<WasiBlobstore>(WASI_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.id.clone()).or_default();

        match workload_storage.get_mut(container_name) {
            Some(container_data) => {
                container_data.objects.remove(&name);
                Ok(Ok(()))
            }
            None => Ok(Err(format!("container '{container_name}' does not exist"))),
        }
    }

    async fn delete_objects(
        &mut self,
        container: Resource<String>,
        names: Vec<ObjectName>,
    ) -> anyhow::Result<Result<(), ContainerError>> {
        let container_name = self.table.get(&container)?;

        let Some(plugin) = self.get_plugin::<WasiBlobstore>(WASI_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.id.clone()).or_default();

        match workload_storage.get_mut(container_name) {
            Some(container_data) => {
                for name in names {
                    container_data.objects.remove(&name);
                }
                Ok(Ok(()))
            }
            None => Ok(Err(format!("container '{container_name}' does not exist"))),
        }
    }

    async fn has_object(
        &mut self,
        container: Resource<String>,
        name: ObjectName,
    ) -> anyhow::Result<Result<bool, ContainerError>> {
        let container_name = self.table.get(&container)?;

        let Some(plugin) = self.get_plugin::<WasiBlobstore>(WASI_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let storage = plugin.storage.read().await;
        let empty_map = HashMap::new();
        let workload_storage = storage.get(&self.id).unwrap_or(&empty_map);

        match workload_storage.get(container_name) {
            Some(container_data) => Ok(Ok(container_data.objects.contains_key(&name))),
            None => Ok(Err(format!("container '{container_name}' does not exist"))),
        }
    }

    async fn object_info(
        &mut self,
        container: Resource<String>,
        name: ObjectName,
    ) -> anyhow::Result<Result<ObjectMetadata, ContainerError>> {
        let container_name = self.table.get(&container)?;

        let Some(plugin) = self.get_plugin::<WasiBlobstore>(WASI_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let storage = plugin.storage.read().await;
        let empty_map = HashMap::new();
        let workload_storage = storage.get(&self.id).unwrap_or(&empty_map);

        match workload_storage.get(container_name) {
            Some(container_data) => match container_data.objects.get(&name) {
                Some(object_data) => Ok(Ok(ObjectMetadata {
                    name: object_data.name.clone(),
                    container: object_data.container.clone(),
                    created_at: object_data.created_at,
                    size: object_data.data.len() as u64,
                })),
                None => Ok(Err(format!("object '{name}' does not exist"))),
            },
            None => Ok(Err(format!("container '{container_name}' does not exist"))),
        }
    }

    async fn clear(
        &mut self,
        container: Resource<String>,
    ) -> anyhow::Result<Result<(), ContainerError>> {
        let container_name = self.table.get(&container)?;

        let Some(plugin) = self.get_plugin::<WasiBlobstore>(WASI_BLOBSTORE_ID) else {
            return Ok(Err("blobstore plugin not available".to_string()));
        };

        let mut storage = plugin.storage.write().await;
        let workload_storage = storage.entry(self.id.clone()).or_default();

        match workload_storage.get_mut(container_name) {
            Some(container_data) => {
                container_data.objects.clear();
                Ok(Ok(()))
            }
            None => Ok(Err(format!("container '{container_name}' does not exist"))),
        }
    }

    async fn drop(&mut self, rep: Resource<String>) -> anyhow::Result<()> {
        // Container resource cleanup - resource table handles this automatically
        tracing::debug!(
            workload_id = self.id,
            resource_id = ?rep,
            "Dropping container resource"
        );
        self.table.delete(rep)?;
        Ok(())
    }
}

impl bindings::wasi::blobstore::container::HostStreamObjectNames for Ctx {
    async fn read_stream_object_names(
        &mut self,
        stream: Resource<StreamObjectNamesHandle>,
        len: u64,
    ) -> anyhow::Result<Result<(Vec<ObjectName>, bool), ContainerError>> {
        let stream_handle = self.table.get_mut(&stream)?;

        let remaining = stream_handle
            .objects
            .len()
            .saturating_sub(stream_handle.position);
        let to_read = (len as usize).min(remaining);

        let mut objects = Vec::new();
        for i in 0..to_read {
            if let Some(obj_name) = stream_handle.objects.get(stream_handle.position + i) {
                objects.push(obj_name.clone());
            }
        }

        stream_handle.position += to_read;
        let is_end = stream_handle.position >= stream_handle.objects.len();

        Ok(Ok((objects, is_end)))
    }

    async fn skip_stream_object_names(
        &mut self,
        stream: Resource<StreamObjectNamesHandle>,
        num: u64,
    ) -> anyhow::Result<Result<(u64, bool), ContainerError>> {
        let stream_handle = self.table.get_mut(&stream)?;

        let remaining = stream_handle
            .objects
            .len()
            .saturating_sub(stream_handle.position);
        let to_skip = (num as usize).min(remaining);

        stream_handle.position += to_skip;
        let is_end = stream_handle.position >= stream_handle.objects.len();

        Ok(Ok((to_skip as u64, is_end)))
    }

    async fn drop(&mut self, rep: Resource<StreamObjectNamesHandle>) -> anyhow::Result<()> {
        // StreamObjectNames resource cleanup
        tracing::debug!(
            workload_id = self.id,
            resource_id = ?rep,
            "Dropping StreamObjectNames resource"
        );
        self.table.delete(rep)?;
        Ok(())
    }
}

impl bindings::wasi::blobstore::types::HostOutgoingValue for Ctx {
    async fn new_outgoing_value(&mut self) -> anyhow::Result<Resource<OutgoingValueHandle>> {
        tracing::debug!(workload_id = self.id, "Creating new OutgoingValue");

        let Some(plugin) = self.get_plugin::<WasiBlobstore>(WASI_BLOBSTORE_ID) else {
            tracing::error!("blobstore plugin not available in new_outgoing_value");
            return Err(anyhow::anyhow!("blobstore plugin not available"));
        };

        let handle = OutgoingValueHandle {
            pipe: MemoryOutputPipe::new(plugin.max_object_size),
            container_name: None,
            object_name: None,
        };

        tracing::debug!(
            workload_id = self.id,
            pipe_capacity = plugin.max_object_size,
            "Created OutgoingValueHandle with MemoryOutputPipe"
        );

        match self.table.push(handle) {
            Ok(resource) => {
                tracing::debug!(
                    workload_id = self.id,
                    resource_id = ?resource,
                    "Successfully pushed OutgoingValueHandle to resource table"
                );
                Ok(resource)
            }
            Err(e) => {
                tracing::error!(
                    workload_id = self.id,
                    error = ?e,
                    "Failed to push OutgoingValueHandle to resource table in new_outgoing_value"
                );
                Err(e.into())
            }
        }
    }

    async fn outgoing_value_write_body(
        &mut self,
        outgoing_value: Resource<OutgoingValueHandle>,
    ) -> anyhow::Result<Result<Resource<bindings::wasi::io0_2_1::streams::OutputStream>, ()>> {
        tracing::debug!(workload_id = self.id, "outgoing_value_write_body called");

        let handle = match self.table.get_mut(&outgoing_value) {
            Ok(h) => {
                tracing::debug!(
                    workload_id = self.id,
                    "Successfully retrieved OutgoingValueHandle from table"
                );
                h
            }
            Err(e) => {
                tracing::error!(
                    workload_id = self.id,
                    error = ?e,
                    "Failed to get OutgoingValueHandle from table"
                );
                return Err(e.into());
            }
        };

        tracing::debug!(
            workload_id = self.id,
            "Creating boxed OutputStream from pipe"
        );

        // Return the pipe as the output stream - this is the same pipe that will be read in finish()
        let boxed: Box<dyn OutputStream> = Box::new(handle.pipe.clone());

        tracing::debug!(
            workload_id = self.id,
            "Attempting to push OutputStream to resource table"
        );

        match self.table.push(boxed) {
            Ok(stream) => {
                tracing::debug!(
                    workload_id = self.id,
                    stream_resource_id = ?stream,
                    "Successfully pushed OutputStream to resource table"
                );
                Ok(Ok(stream))
            }
            Err(e) => {
                tracing::error!(
                    workload_id = self.id,
                    error = ?e,
                    error_type = std::any::type_name::<anyhow::Error>(),
                    "Failed to push OutputStream to resource table - this is likely the TryFromIntError source"
                );
                Err(e.into())
            }
        }
    }

    async fn finish(
        &mut self,
        outgoing_value: Resource<OutgoingValueHandle>,
    ) -> anyhow::Result<Result<(), BlobstoreError>> {
        tracing::debug!(workload_id = self.id, "finish() called for OutgoingValue");

        let handle = self.table.delete(outgoing_value)?;

        tracing::debug!(
            container_name = ?handle.container_name,
            object_name = ?handle.object_name,
            "Retrieved OutgoingValueHandle in finish()"
        );

        // If we have container and object names, perform the actual write
        if let (Some(container_name), Some(object_name)) =
            (&handle.container_name, &handle.object_name)
        {
            let Some(plugin) = self.get_plugin::<WasiBlobstore>(WASI_BLOBSTORE_ID) else {
                tracing::error!("blobstore plugin not available in finish()");
                return Ok(Err("blobstore plugin not available".to_string()));
            };

            // Get the data from the pipe
            let data_bytes = handle.pipe.contents();

            tracing::debug!(
                container = container_name,
                object = object_name,
                pipe_data_size = data_bytes.len(),
                workload_id = self.id,
                "Retrieved data from pipe in finish()"
            );

            let mut storage = plugin.storage.write().await;
            let workload_storage = storage.entry(self.id.clone()).or_default();

            match workload_storage.get_mut(container_name) {
                Some(container_data) => {
                    let object_data = ObjectData {
                        name: object_name.clone(),
                        container: container_name.clone(),
                        data: data_bytes.to_vec(),
                        created_at: WasiBlobstore::get_timestamp(),
                    };
                    container_data
                        .objects
                        .insert(object_name.clone(), object_data);

                    tracing::debug!(
                        container = container_name,
                        object = object_name,
                        size = data_bytes.len(),
                        "Stored object data to container"
                    );
                }
                None => {
                    tracing::error!(
                        container = container_name,
                        workload_id = self.id,
                        "Container does not exist in finish()"
                    );
                    return Ok(Err(format!("container '{container_name}' does not exist")));
                }
            }
        } else {
            tracing::warn!(
                workload_id = self.id,
                "finish() called without container/object names set"
            );
        }

        Ok(Ok(()))
    }

    async fn drop(&mut self, rep: Resource<OutgoingValueHandle>) -> anyhow::Result<()> {
        tracing::debug!(
            workload_id = self.id,
            resource_id = ?rep,
            "Dropping OutgoingValue resource"
        );
        self.table.delete(rep)?;
        Ok(())
    }
}

impl bindings::wasi::blobstore::types::HostIncomingValue for Ctx {
    async fn incoming_value_consume_sync(
        &mut self,
        incoming_value: Resource<IncomingValueHandle>,
    ) -> anyhow::Result<Result<Vec<u8>, BlobstoreError>> {
        let data = self.table.delete(incoming_value)?;

        tracing::debug!(
            workload_id = self.id,
            data_size = data.len(),
            "incoming_value_consume_sync returning data"
        );

        Ok(Ok(data))
    }

    async fn incoming_value_consume_async(
        &mut self,
        incoming_value: Resource<IncomingValueHandle>,
    ) -> anyhow::Result<
        Result<Resource<bindings::wasi::blobstore::types::IncomingValueAsyncBody>, BlobstoreError>,
    > {
        let data = self.table.get(&incoming_value)?;

        tracing::debug!(
            workload_id = self.id,
            data_size = data.len(),
            "incoming_value_consume_async creating MemoryInputPipe with data"
        );

        let stream: Box<dyn InputStream> = Box::new(MemoryInputPipe::new(data.clone()));
        let stream = self.table.push(stream)?;

        tracing::debug!(
            workload_id = self.id,
            "incoming_value_consume_async created stream resource"
        );

        Ok(Ok(stream))
    }

    async fn size(&mut self, incoming_value: Resource<IncomingValueHandle>) -> anyhow::Result<u64> {
        let data = self.table.get(&incoming_value)?;
        Ok(data.len() as u64)
    }

    async fn drop(&mut self, rep: Resource<IncomingValueHandle>) -> anyhow::Result<()> {
        tracing::debug!(
            workload_id = self.id,
            resource_id = ?rep,
            "Dropping IncomingValue resource"
        );
        self.table.delete(rep)?;
        Ok(())
    }
}

// Note: wasi:io interface implementations are handled automatically by wasmtime-wasi
// when setting up the Ctx during runtime initialization. The bindgen-generated
// traits are sealed and can only be implemented on &mut _T types.

// Implement the main types Host trait that combines all resource types
impl bindings::wasi::blobstore::types::Host for Ctx {}

// Implement the main container Host trait that combines all resource types
impl bindings::wasi::blobstore::container::Host for Ctx {}

#[async_trait::async_trait]
impl HostPlugin for WasiBlobstore {
    fn id(&self) -> &'static str {
        WASI_BLOBSTORE_ID
    }
    fn world(&self) -> WitWorld {
        WitWorld {
            imports: HashSet::from([WitInterface::from(
                "wasi:blobstore/blobstore,container,types@0.2.0-draft",
            )]),
            ..Default::default()
        }
    }

    async fn bind_component(
        &self,
        workload_handle: &mut WorkloadComponent,
        interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        // Check if any of the interfaces are wasi:blobstore related
        let has_blobstore = interfaces
            .iter()
            .any(|i| i.namespace == "wasi" && i.package == "blobstore");

        if !has_blobstore {
            tracing::warn!(
                "WasiBlobstore plugin requested for non-wasi:blobstore interface(s): {:?}",
                interfaces
            );
            return Ok(());
        }

        // Add blobstore interfaces to the workload's linker
        // Note: wasi:io interfaces are already added by wasmtime_wasi::add_to_linker_async()
        // in the engine initialization, so we only need to add the blobstore-specific interfaces
        tracing::debug!(
            workload_id = workload_handle.id(),
            "Adding blobstore interfaces to linker for workload"
        );
        let linker = workload_handle.linker();

        bindings::wasi::blobstore::blobstore::add_to_linker(linker, |ctx| ctx)?;
        bindings::wasi::blobstore::container::add_to_linker(linker, |ctx| ctx)?;
        bindings::wasi::blobstore::types::add_to_linker(linker, |ctx| ctx)?;

        let id = workload_handle.id();

        tracing::debug!(
            workload_id = id,
            "Successfully added blobstore interfaces to linker for workload"
        );

        // Initialize storage for this workload
        let mut storage = self.storage.write().await;
        storage.insert(id.to_string(), HashMap::new());

        tracing::debug!("WasiBlobstore plugin bound to workload '{id}'");

        Ok(())
    }

    async fn unbind_workload(
        &self,
        workload_handle: ResolvedWorkload,
        _interfaces: std::collections::HashSet<crate::wit::WitInterface>,
    ) -> anyhow::Result<()> {
        let id = workload_handle.id();
        // Clean up storage for this workload
        let mut storage = self.storage.write().await;
        storage.remove(id);

        tracing::debug!("WasiBlobstore plugin unbound from workload '{id}'");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wasi_blobstore_creation() {
        let blobstore = WasiBlobstore::new(None);
        assert!(blobstore.storage.try_read().is_ok());
    }

    #[test]
    fn test_get_timestamp() {
        let timestamp = WasiBlobstore::get_timestamp();
        assert!(timestamp > 0);
    }

    #[test]
    fn test_object_data_creation() {
        let data = ObjectData {
            name: "test.txt".to_string(),
            container: "test-container".to_string(),
            data: b"hello world".to_vec(),
            created_at: WasiBlobstore::get_timestamp(),
        };

        assert_eq!(data.name, "test.txt");
        assert_eq!(data.container, "test-container");
        assert_eq!(data.data, b"hello world");
        assert!(data.created_at > 0);
    }

    #[test]
    fn test_container_data_creation() {
        let container = ContainerData {
            name: "test-container".to_string(),
            created_at: WasiBlobstore::get_timestamp(),
            objects: HashMap::new(),
        };

        assert_eq!(container.name, "test-container");
        assert!(container.created_at > 0);
        assert!(container.objects.is_empty());
    }

    #[tokio::test]
    async fn test_storage_operations() {
        let blobstore = WasiBlobstore::new(None);

        // Test write access
        {
            let mut storage = blobstore.storage.write().await;
            storage.insert("workload1".to_string(), HashMap::new());
        }

        // Test read access
        {
            let storage = blobstore.storage.read().await;
            assert!(storage.contains_key("workload1"));
        }
    }
}
