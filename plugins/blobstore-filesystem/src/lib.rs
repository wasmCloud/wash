//! This component implements `wasi:blobstore` in terms of `wasi:filesystem`, allowing
//! components to use the blobstore API with a backing filesystem for development and testing.
/// A container backed by a filesystem directory
pub type Container = String;

/// A stream of object names backed by a directory iterator
pub struct StreamObjectNames {
    pub container_name: String,
    pub objects: Vec<String>,
    pub position: usize,
}

mod bindings {
    use super::Component;

    wit_bindgen::generate!({
        path: "../../wit",
        world: "blobstore-host",
        with: {
            "wasi:io/error@0.2.1": ::wasi::io::error,
            "wasi:io/poll@0.2.1": ::wasi::io::poll,
            "wasi:io/streams@0.2.1": ::wasi::io::streams,
            "wasi:blobstore/blobstore@0.2.0-draft": generate,
            "wasi:blobstore/container@0.2.0-draft": generate,
            "wasi:blobstore/types@0.2.0-draft": generate,
        },
    });

    export!(Component);
}

use wasi::filesystem::types::{Descriptor, DescriptorFlags, OpenFlags, PathFlags};

use crate::bindings::exports::wasi::blobstore::container::OutgoingValueBorrow;
use crate::bindings::exports::wasi::blobstore::types::{
    ContainerMetadata, IncomingValue, IncomingValueSyncBody, ObjectId, ObjectMetadata, ObjectName,
    OutgoingValue,
};
use crate::bindings::exports::wasi::blobstore::types::{GuestIncomingValue, GuestOutgoingValue};
use crate::bindings::exports::wasi::blobstore::{blobstore, container};
use crate::bindings::wasmcloud::wash::types::{
    Command, CredentialType, HookType, Metadata, Runner,
};

/// Type alias to return wasi streams
pub type OutgoingValueStream = ::wasi::io::streams::OutputStream;
pub type IncomingValueStream = ::wasi::io::streams::InputStream;

struct Component;

impl bindings::exports::wasmcloud::wash::plugin::Guest for Component {
    /// Called by wash to retrieve the plugin metadata
    fn info() -> Metadata {
        Metadata {
            id: "dev.wasmcloud.wasi-filesystem-to-blobstore".to_string(),
            name: "wasi-filesystem-to-blobstore".to_string(),
            description: "Implements the wasi:blobstore API using the wasi:filesystem API"
                .to_string(),
            contact: "wasmCloud Team".to_string(),
            url: "https://github.com/wasmcloud/wash".to_string(),
            license: "Apache-2.0".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            default_command: None,
            commands: vec![],
            hooks: Some(vec![HookType::DevRegister]),
            credentials: None,
        }
    }

    // All of these functions aren't valid for this type of plugin
    fn initialize(_: Runner) -> anyhow::Result<(), ()> {
        Err(())
    }
    fn run(_: Runner, _: Command) -> anyhow::Result<(), ()> {
        Err(())
    }
    fn hook(_: Runner, _: HookType) -> anyhow::Result<(), ()> {
        Err(())
    }
    fn authorize(_: Runner, _: CredentialType, _: Option<String>) -> anyhow::Result<String, ()> {
        Err(())
    }
}

/// Get the root directory of the filesystem. The implementation of `wasi:blobstore` for wash
/// assumes that the root directory is the first preopened directory provided by the WASI runtime.
fn get_root_dir() -> Result<(Descriptor, String), String> {
    let dirs = ::wasi::filesystem::preopens::get_directories();
    if let Some((fdir, path)) = dirs.into_iter().take(1).next() {
        Ok((fdir, path))
    } else {
        Err("No root directory found".to_string())
    }
}

impl bindings::exports::wasi::blobstore::blobstore::Guest for Component {
    fn create_container(container: String) -> Result<blobstore::Container, String> {
        eprintln!("Creating container: {container}");
        let (root_dir, _root_path) = get_root_dir()?;
        root_dir
            .create_directory_at(&container)
            .map_err(|e| e.to_string())?;

        // Return a new container instance
        Ok(blobstore::Container::new(container.clone()))
    }

    fn get_container(container: String) -> Result<blobstore::Container, String> {
        // Ensure the container exists
        if !Component::container_exists(container.clone())? {
            return Err(format!("Container '{container}' does not exist"));
        }

        Ok(blobstore::Container::new(container))
    }

    fn delete_container(container: String) -> Result<(), String> {
        let (root_dir, _root_path) = get_root_dir()?;
        // Attempt to remove the directory
        root_dir
            .remove_directory_at(&container)
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    fn container_exists(container: String) -> Result<bool, String> {
        let (root_dir, _root_path) = get_root_dir()?;
        // Check if the directory exists
        match root_dir.open_at(
            PathFlags::empty(),
            &container,
            OpenFlags::empty(),
            DescriptorFlags::empty(),
        ) {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    fn copy_object(_src: ObjectId, _dst: ObjectId) -> Result<(), String> {
        // let src_container = Component::get_container(src.container.clone())?;
        // let dst_container = Component::get_container(dst.container.clone())?;
        todo!()
    }

    fn move_object(_src: ObjectId, _dst: ObjectId) -> Result<(), String> {
        todo!()
    }
}

impl bindings::exports::wasi::blobstore::container::Guest for Component {
    type Container = crate::Container;
    type StreamObjectNames = crate::StreamObjectNames;
}

impl bindings::exports::wasi::blobstore::types::Guest for Component {
    type OutgoingValue = OutgoingValueStream;
    type IncomingValue = IncomingValueStream;
}

impl container::GuestContainer for crate::Container {
    /// Returns container name
    fn name(&self) -> Result<String, container::Error> {
        Ok(self.clone())
    }

    /// Returns container metadata
    fn info(&self) -> Result<ContainerMetadata, container::Error> {
        todo!()
    }

    /// Retrieves an object or portion of an object, as a resource.
    /// Start and end offsets are inclusive.
    /// Once a data-blob resource has been created, the underlying bytes are held by the blobstore service for the lifetime
    /// of the data-blob resource, even if the object they came from is later deleted.
    fn get_data(
        &self,
        name: ObjectName,
        start: u64,
        _end: u64,
    ) -> Result<IncomingValue, container::Error> {
        let (root_dir, _path) = get_root_dir().expect("should get root dir"); //TODO: handle gracefully
                                                                              // Open the container directory
        let dir = root_dir
            .open_at(
                PathFlags::empty(),
                &self,
                OpenFlags::empty(),
                DescriptorFlags::empty(),
            )
            .map_err(|e| container::Error::from(e.to_string()))?;

        let file = dir
            .open_at(
                PathFlags::empty(),
                &name,
                OpenFlags::empty(),
                DescriptorFlags::empty(),
            )
            .map_err(|e| container::Error::from(e.to_string()))?;

        let stream = file
            .read_via_stream(start)
            .map_err(|e| container::Error::from(e.to_string()))?;

        Ok(IncomingValue::new(stream))
    }

    /// Creates or replaces an object with the data blob.
    fn write_data(
        &self,
        name: ObjectName,
        data: OutgoingValueBorrow,
    ) -> Result<(), container::Error> {
        todo!()
    }

    /// Returns list of objects in the container. Order is undefined.
    fn list_objects(&self) -> Result<container::StreamObjectNames, container::Error> {
        todo!()
    }

    /// Deletes object.
    /// Does not return error if object did not exist.
    fn delete_object(&self, name: ObjectName) -> Result<(), container::Error> {
        todo!()
    }

    /// Deletes multiple objects in the container
    fn delete_objects(&self, names: Vec<ObjectName>) -> Result<(), container::Error> {
        todo!()
    }

    /// Returns true if the object exists in this container
    fn has_object(&self, name: ObjectName) -> Result<bool, container::Error> {
        todo!()
    }

    /// Returns metadata for the object
    fn object_info(&self, name: ObjectName) -> Result<ObjectMetadata, container::Error> {
        todo!()
    }

    /// Removes all objects within the container, leaving the container empty.
    fn clear(&self) -> Result<(), container::Error> {
        todo!()
    }
}

impl container::GuestStreamObjectNames for crate::StreamObjectNames {
    /// Reads the next number of objects from the stream
    ///
    /// This function returns the list of objects read, and a boolean indicating if the end of the stream was reached.
    fn read_stream_object_names(
        &self,
        len: u64,
    ) -> Result<(Vec<ObjectName>, bool), container::Error> {
        todo!()
    }

    /// Skip the next number of objects in the stream
    ///
    /// This function returns the number of objects skipped, and a boolean indicating if the end of the stream was reached.
    fn skip_stream_object_names(&self, num: u64) -> Result<(u64, bool), container::Error> {
        todo!()
    }
}

impl GuestOutgoingValue for OutgoingValueStream {
    /// Create a new outgoing value.
    fn new_outgoing_value() -> OutgoingValue {
        // OutgoingValue::new(OutputStream::)
        // OutputStream::new_outgoing_value()
        todo!()
    }

    /// Returns a stream for writing the value contents.
    ///
    /// The returned `output-stream` is a child resource: it must be dropped
    /// before the parent `outgoing-value` resource is dropped (or finished),
    /// otherwise the `outgoing-value` drop or `finish` will trap.
    ///
    /// Returns success on the first call: the `output-stream` resource for
    /// this `outgoing-value` may be retrieved at most once. Subsequent calls
    /// will return error.
    #[allow(async_fn_in_trait)]
    fn outgoing_value_write_body(&self) -> Result<::wasi::io::streams::OutputStream, ()> {
        todo!()
    }

    /// Finalize an outgoing value. This must be
    /// called to signal that the outgoing value is complete. If the `outgoing-value`
    /// is dropped without calling `outgoing-value.finalize`, the implementation
    /// should treat the value as corrupted.
    #[allow(async_fn_in_trait)]
    fn finish(this: OutgoingValue) -> Result<(), String> {
        todo!()
    }
}

use std::io::Read;
impl GuestIncomingValue for IncomingValueStream {
    /// Consume the incoming value synchronously.
    fn incoming_value_consume_sync(this: IncomingValue) -> Result<IncomingValueSyncBody, String> {
        // let mut buf = vec![];

        // TODO: basically just need type assertions to make it a stream
        // this.into_inner()
        //     .read_to_end(&mut buf)
        //     .map_err(|e| e.to_string())?;
        todo!()
    }

    /// Consume the incoming value asynchronously.
    fn incoming_value_consume_async(
        this: IncomingValue,
    ) -> Result<crate::bindings::exports::wasi::blobstore::types::IncomingValueAsyncBody, String>
    {
        todo!()
    }

    /// Get the size of the incoming value.
    fn size(&self) -> u64 {
        todo!()
    }
}
