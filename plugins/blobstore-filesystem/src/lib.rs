//! This component implements `wasi:blobstore` in terms of `wasi:filesystem`, allowing
//! components to use the blobstore API with a backing filesystem for development and testing.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Read;

use wasi::filesystem::types::{Descriptor, DescriptorFlags, DescriptorType, OpenFlags, PathFlags};

use crate::bindings::exports::wasi::blobstore::types::{
    ContainerMetadata, IncomingValue, IncomingValueSyncBody, ObjectId, ObjectMetadata, ObjectName,
    OutgoingValue, OutgoingValueBorrow,
};
use crate::bindings::exports::wasi::blobstore::types::{GuestIncomingValue, GuestOutgoingValue};
use crate::bindings::exports::wasi::blobstore::{blobstore, container};
use crate::bindings::wasi::logging::logging::{Level, log};

/// Generated WIT bindings
pub mod bindings;
/// `wasmcloud:wash/plugin` implementation
pub mod plugin;

/// Outgoing value stream type for uploading files
pub type OutgoingValueStream = RefCell<UploadFile>;

#[derive(Default)]
pub struct UploadFile {
    pub src_path: Option<String>,
    pub dst_container: Option<String>,
    pub dst_path: Option<String>,
}

impl UploadFile {
    /// Returns a tuple of the source path, destination container, and destination path.
    pub fn into_parts(self) -> (Option<String>, Option<String>, Option<String>) {
        (self.src_path, self.dst_container, self.dst_path)
    }
}

/// Incoming value stream type for downloading files
pub struct IncomingValueStream {
    pub stream: ::wasi::io::streams::InputStream,
    pub size: u64,
}

/// A container backed by a filesystem directory
pub type Container = String;

/// A stream of object names backed by a directory iterator
pub type StreamObjectNames = RefCell<ObjectNames>;

pub struct ObjectNames {
    pub container_name: String,
    pub objects: VecDeque<String>,
}

impl bindings::exports::wasi::blobstore::container::Guest for Component {
    type Container = crate::Container;
    type StreamObjectNames = crate::StreamObjectNames;
}

impl bindings::exports::wasi::blobstore::types::Guest for Component {
    type OutgoingValue = OutgoingValueStream;
    type IncomingValue = IncomingValueStream;
}

struct Component;

/// Get the root directory of the filesystem. The implementation of `wasi:blobstore` for wash
/// assumes that the root directory is the last preopened directory provided by the WASI runtime.
///
/// This is the only function that interacts with wasi:filesystem directly as it's required
/// to get the root directory for all operations.
fn get_root_dir() -> Result<(Descriptor, String), String> {
    let mut dirs = ::wasi::filesystem::preopens::get_directories();
    if dirs.len() > 1 {
        log(
            Level::Warn,
            "",
            "multiple preopened directories found, using the last one provided",
        );
    }

    if let Some(dir) = dirs.pop() {
        Ok((dir.0, dir.1))
    } else {
        Err("No preopened directories found".to_string())
    }
}

impl bindings::exports::wasi::blobstore::blobstore::Guest for Component {
    fn create_container(container: String) -> Result<blobstore::Container, String> {
        log(
            Level::Debug,
            &format!("container={container}"),
            "creating container",
        );
        let (root_dir, root_path) = get_root_dir()?;

        root_dir
            .create_directory_at(&container)
            .map_err(|e| e.to_string())?;

        log(
            Level::Debug,
            &format!("container={container}"),
            &format!("created container at {root_path:?}"),
        );

        // Return a new container instance
        Ok(blobstore::Container::new(container))
    }

    fn get_container(container: String) -> Result<blobstore::Container, String> {
        log(
            Level::Debug,
            &format!("container={container}"),
            "getting container",
        );
        // Ensure the container exists
        if !Component::container_exists(container.clone())? {
            return Err(format!("container={container}: container does not exist"));
        }

        Ok(blobstore::Container::new(container))
    }

    fn delete_container(container: String) -> Result<(), String> {
        log(
            Level::Debug,
            &format!("container={container}"),
            "deleting container",
        );
        let (root_dir, _root_path) = get_root_dir()?;

        if !Component::container_exists(container.clone())? {
            return Ok(()); // Container does not exist, nothing to delete
        }

        // Remove all objects in the container
        container::GuestContainer::clear(&container)
            .map_err(|e| format!("container={container}: failed to clear container: {e}"))?;

        // Attempt to remove the directory
        root_dir
            .remove_directory_at(&container)
            .map_err(|e| format!("container={container}: failed to delete container: {e}"))
    }

    fn container_exists(container: String) -> Result<bool, String> {
        log(
            Level::Debug,
            &format!("container={container}"),
            "checking if container exists",
        );
        let (root_dir, _root_path) = get_root_dir()?;

        match root_dir.stat_at(PathFlags::empty(), &container) {
            Ok(stat) => {
                // Container exists if it is a directory
                Ok(stat.type_ == DescriptorType::Directory)
            }
            Err(e) => {
                // If the error is NotFound, the container does not exist
                if e == wasi::filesystem::types::ErrorCode::NoEntry {
                    Ok(false)
                } else {
                    Err(format!("container={container}: {e}"))
                }
            }
        }
    }

    fn copy_object(src: ObjectId, dst: ObjectId) -> Result<(), String> {
        log(
            Level::Debug,
            &format!(
                "source={}/{},destination={}/{}",
                src.container, src.object, dst.container, dst.object
            ),
            "copying object",
        );
        let (root_dir, _root_path) = get_root_dir()?;

        // Build source and destination paths
        let src_path = format!("{}/{}", src.container, src.object);

        // Open the source file
        let src_file = root_dir
            .open_at(
                PathFlags::empty(),
                &src_path,
                OpenFlags::empty(),
                DescriptorFlags::READ,
            )
            .map_err(|e| format!("error={e}, failed to open source object"))?;

        // Open stream for reading the source file
        let mut src_stream = src_file
            .read_via_stream(0)
            .map_err(|e| format!("error={e}, failed to create read stream"))?;

        let dst_container = root_dir
            .open_at(
                PathFlags::empty(),
                &dst.container,
                OpenFlags::DIRECTORY,
                DescriptorFlags::MUTATE_DIRECTORY,
            )
            .map_err(|e| format!("error={e}, failed to open destination container"))?;

        // Create or truncate the destination file
        let dst_file = dst_container
            .open_at(
                PathFlags::empty(),
                &dst.object,
                OpenFlags::CREATE | OpenFlags::TRUNCATE,
                DescriptorFlags::WRITE,
            )
            .map_err(|e| format!("error={e}, failed to open destination object"))?;

        // Write the buffer to the destination file
        let mut dst_stream = dst_file
            .write_via_stream(0)
            .map_err(|e| format!("error={e}, failed to create write stream"))?;

        std::io::copy(&mut src_stream, &mut dst_stream)
            .map(|_| ())
            .map_err(|e| format!("error={e}, failed to copy data"))
    }

    fn move_object(src: ObjectId, dst: ObjectId) -> Result<(), String> {
        log(
            Level::Debug,
            &format!(
                "source={}/{},destination={}/{}",
                src.container, src.object, dst.container, dst.object
            ),
            "moving object",
        );
        let (root_dir, _root_path) = get_root_dir()?;
        let src_path = format!("{}/{}", src.container, src.object);

        let container = root_dir
            .open_at(
                PathFlags::empty(),
                &dst.container,
                OpenFlags::DIRECTORY,
                DescriptorFlags::MUTATE_DIRECTORY,
            )
            .map_err(|e| e.to_string())?;

        // Rename upload file to the destination path
        root_dir
            .rename_at(&src_path, &container, &dst.object)
            .map_err(|e| format!("failed to rename file: {e}"))
    }
}

impl container::GuestContainer for crate::Container {
    /// Returns container name
    fn name(&self) -> Result<String, container::Error> {
        Ok(self.clone())
    }

    /// Returns container metadata
    fn info(&self) -> Result<ContainerMetadata, container::Error> {
        let metadata =
            std::fs::metadata(&self).map_err(|e| container::Error::from(e.to_string()))?;

        Ok(ContainerMetadata {
            name: self.name()?,
            created_at: metadata
                .created()
                .and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Unsupported, e))
                })
                .unwrap_or(0),
        })
    }

    /// Retrieves an object or portion of an object, as a resource.
    /// Start and end offsets are inclusive.
    /// Once a data-blob resource has been created, the underlying bytes are held by the blobstore service for the lifetime
    /// of the data-blob resource, even if the object they came from is later deleted.
    fn get_data(
        &self,
        name: ObjectName,
        start: u64,
        end: u64,
    ) -> Result<IncomingValue, container::Error> {
        let (root_dir, _path) = get_root_dir()?;
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

        // File size is minimum of the file size and the requested range
        let size = file
            .stat()
            .map_err(|e| container::Error::from(e.to_string()))?
            .size
            .min(end - start);

        let stream = file
            .read_via_stream(start)
            .map_err(|e| container::Error::from(e.to_string()))?;

        Ok(IncomingValue::new(IncomingValueStream { stream, size }))
    }

    /// Creates or replaces an object with the data blob.
    /// This simply assigns a destination path to the `OutgoingValueBorrow` stream. Finishing
    /// the [`OutgoingValue`] will write the data to the destination path.
    fn write_data(
        &self,
        name: ObjectName,
        data: OutgoingValueBorrow,
    ) -> Result<(), container::Error> {
        let outgoing_stream: &OutgoingValueStream = data.get();
        let mut upload_file = outgoing_stream.borrow_mut();

        // Write the destination path
        upload_file.dst_path = Some(name);
        upload_file.dst_container = Some(self.name()?);

        Ok(())
    }

    /// Returns list of objects in the container. Order is undefined.
    fn list_objects(&self) -> Result<container::StreamObjectNames, container::Error> {
        let container_name = self.name()?;

        let entries = std::fs::read_dir(self).map_err(|e| container::Error::from(e.to_string()))?;
        let objects = entries
            .filter_map(|entry| entry.ok().and_then(|e| e.file_name().into_string().ok()))
            .collect::<VecDeque<_>>();

        Ok(container::StreamObjectNames::new(RefCell::new(
            ObjectNames {
                container_name,
                objects,
            },
        )))
    }

    /// Deletes object.
    /// Does not return error if object did not exist.
    fn delete_object(&self, name: ObjectName) -> Result<(), container::Error> {
        self.delete_objects(vec![name])
    }

    fn delete_objects(&self, names: Vec<ObjectName>) -> Result<(), container::Error> {
        let (root_dir, _root_path) = get_root_dir().map_err(|e| container::Error::from(e))?;

        // Open the container directory as a Descriptor
        let container_dir = root_dir
            .open_at(
                PathFlags::empty(),
                &self.name()?,
                OpenFlags::DIRECTORY,
                DescriptorFlags::MUTATE_DIRECTORY,
            )
            .map_err(|e| container::Error::from(e.to_string()))?;

        // Try to remove the file; ignore NotFound errors
        for name in names {
            if let Err(e) = container_dir.unlink_file_at(&name) {
                if e == wasi::filesystem::types::ErrorCode::IsDirectory {
                    return Err(container::Error::from(
                        "Cannot delete a directory using delete_object".to_string(),
                    ));
                }
            }
        }

        Ok(())
    }

    fn has_object(&self, name: ObjectName) -> Result<bool, container::Error> {
        let (root_dir, _root_path) = get_root_dir()?;

        let container = root_dir
            .open_at(
                PathFlags::empty(),
                &self.name()?,
                OpenFlags::DIRECTORY,
                DescriptorFlags::empty(),
            )
            .map_err(|e| container::Error::from(e.to_string()))?;

        // TODO(ISSUE#5): Decide whether or not to follow symlinks
        Ok(container.stat_at(PathFlags::empty(), &name).is_ok())
    }

    fn object_info(&self, name: ObjectName) -> Result<ObjectMetadata, container::Error> {
        let container = self.name()?;

        let (root_dir, _root_path) = get_root_dir()?;

        // Open the container directory
        let container_dir = root_dir
            .open_at(
                PathFlags::empty(),
                &container,
                OpenFlags::DIRECTORY,
                DescriptorFlags::empty(),
            )
            .map_err(|e| container::Error::from(e.to_string()))?;

        let meta = container_dir
            .stat_at(PathFlags::empty(), &name)
            .map_err(|e| container::Error::from(e.to_string()))?;

        Ok(ObjectMetadata {
            container,
            name,
            size: meta.size,
            // Sadly there isn't a way to get the creation time of a file in WASI
            created_at: meta.data_modification_timestamp.unwrap().seconds,
        })
    }

    fn clear(&self) -> Result<(), container::Error> {
        let (root_dir, _root_path) = get_root_dir()?;
        // Open the container directory
        let container = root_dir
            .open_at(
                PathFlags::empty(),
                &self.name()?,
                OpenFlags::DIRECTORY,
                DescriptorFlags::MUTATE_DIRECTORY,
            )
            .map_err(|e| container::Error::from(e.to_string()))?;

        let dir_entries = container
            .read_directory()
            .map_err(|e| container::Error::from(e.to_string()))?;
        while let Ok(Some(dir_entry)) = dir_entries.read_directory_entry() {
            if dir_entry.type_ == DescriptorType::RegularFile {
                // Attempt to remove the file; ignore NoEntry errors
                if let Err(e) = container.unlink_file_at(&dir_entry.name) {
                    if e != wasi::filesystem::types::ErrorCode::NoEntry {
                        return Err(container::Error::from(e.to_string()));
                    }
                }
            } else if dir_entry.type_ == DescriptorType::Directory {
                // Recursively delete directories
                container::GuestContainer::clear(&dir_entry.name)
                    .map_err(|e| container::Error::from(e.to_string()))?;
            }
        }

        Ok(())
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
        let mut stream = self.borrow_mut();

        let mut objects = Vec::with_capacity(len as usize);
        while let Some(name) = stream.objects.pop_front() {
            objects.push(name);
        }

        Ok((objects, stream.objects.is_empty()))
    }

    /// Skip the next number of objects in the stream
    ///
    /// This function returns the number of objects skipped, and a boolean indicating if the end of the stream was reached.
    fn skip_stream_object_names(&self, num: u64) -> Result<(u64, bool), container::Error> {
        let mut stream = self.borrow_mut();

        let mut skipped = 0;
        for _ in 0..num {
            // Stream is empty
            if let None = stream.objects.pop_front() {
                break;
            }
            skipped += 1
        }

        Ok((skipped, stream.objects.is_empty()))
    }
}

impl GuestOutgoingValue for OutgoingValueStream {
    /// Create a new outgoing value.
    fn new_outgoing_value() -> OutgoingValue {
        OutgoingValue::new(RefCell::new(UploadFile::default()))
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
    fn outgoing_value_write_body(&self) -> Result<::wasi::io::streams::OutputStream, ()> {
        let (root_dir, _root_path) = get_root_dir().map_err(|_| ())?;

        // Generate a random file name for the upload file
        let randomized = wasi::random::insecure::get_insecure_random_bytes(6);
        let random_number = randomized
            .iter()
            .fold(0u64, |acc, &b| (acc << 8) | b as u64);
        let random_str = format!("{:06x}", random_number % 0x1000000);
        let src_path = format!("upload_{random_str}.bin");

        // Open a stream for writing to the temporary upload file path
        let f = root_dir
            .open_at(
                PathFlags::empty(),
                &src_path,
                OpenFlags::CREATE | OpenFlags::TRUNCATE,
                DescriptorFlags::WRITE,
            )
            .map_err(|_| ())?;
        let stream = f.write_via_stream(0).map_err(|_| ())?;

        let mut upload_file = self.borrow_mut();
        upload_file.src_path = Some(src_path);

        Ok(stream)
    }

    /// Finalize an outgoing value. This must be
    /// called to signal that the outgoing value is complete. If the `outgoing-value`
    /// is dropped without calling `outgoing-value.finalize`, the implementation
    /// should treat the value as corrupted.
    fn finish(this: OutgoingValue) -> Result<(), String> {
        let stream: OutgoingValueStream = this.into_inner();
        let upload_file: UploadFile = stream.into_inner();

        let (Some(src_path), Some(dst_container), Some(dst_path)) = upload_file.into_parts() else {
            return Err(
                "outgoing-value was not properly initialized or passed to container.write-data"
                    .to_string(),
            );
        };

        let (root_dir, _root_path) =
            get_root_dir().map_err(|e| format!("failed to get root directory: {e}"))?;

        let container = root_dir
            .open_at(
                PathFlags::empty(),
                &dst_container,
                OpenFlags::DIRECTORY,
                DescriptorFlags::MUTATE_DIRECTORY,
            )
            .map_err(|e| e.to_string())?;

        // Rename upload file to the destination path
        root_dir
            .rename_at(&src_path, &container, &dst_path)
            .map_err(|e| format!("failed to rename file: {e}"))?;

        Ok(())
    }
}

impl GuestIncomingValue for IncomingValueStream {
    /// Consume the incoming value synchronously.
    fn incoming_value_consume_sync(this: IncomingValue) -> Result<IncomingValueSyncBody, String> {
        let mut incoming: IncomingValueStream = this.into_inner();
        let mut buf = Vec::with_capacity(incoming.size as usize);
        incoming
            .stream
            .read_exact(&mut buf)
            .map_err(|e| e.to_string())?;
        Ok(buf)
    }

    /// Consume the incoming value asynchronously.
    fn incoming_value_consume_async(
        this: IncomingValue,
    ) -> Result<crate::bindings::exports::wasi::blobstore::types::IncomingValueAsyncBody, String>
    {
        let incoming: IncomingValueStream = this.into_inner();
        Ok(incoming.stream)
    }

    /// Get the size of the incoming value.
    fn size(&self) -> u64 {
        self.size
    }
}
