//! The `lattice_artifacts` module contains functionality relating to reading and writing
//! actor module bytes to and from a NATS object store corresponding to a lattice.
//!
use anyhow::{anyhow, Result};
use async_nats::jetstream::object_store::{Config, ObjectStore};
use tokio::io::AsyncReadExt;

fn bucket_name(lattice_id: &str) -> String {
    format!("ARTIFACT_{}", lattice_id)
}

/// Creates a new artifact store named ARTIFACT_{lattice_id} using the default
/// options (file-backed with no limitations) or, if one already exists, returns
/// a reference to that object store.
pub async fn create_or_reuse_store(
    nc: async_nats::Client,
    lattice_id: &str,
) -> Result<ObjectStore> {
    let js = async_nats::jetstream::new(nc);
    let bucket = bucket_name(lattice_id);

    match js.get_object_store(&bucket).await {
        Ok(os) => Ok(os),
        Err(_) => js
            .create_object_store(Config {
                bucket,
                ..Default::default()
            })
            .await
            .map_err(|e| anyhow!("Failed to create store: {}", e)),
    }
}

/// Writes an arbitrary set of bytes (which should correspond to an actor's raw .wasm binary) to the
/// object store
pub async fn write_to_store(store: ObjectStore, public_key: &str, bytes: Vec<u8>) -> Result<()> {
    store
        .put(public_key, &mut bytes.as_slice())
        .await
        .map_err(|e| anyhow!("Failed to write to artifact store: {}", e))
        .map(|_| ())
}

/// Removes an item from the artifact store
pub async fn remove_item_from_store(store: ObjectStore, public_key: &str) -> Result<()> {
    store
        .delete(public_key)
        .await
        .map_err(|e| anyhow!("Failed to delete item from artifact store: {}", e))
}

pub async fn get_item_from_store(store: ObjectStore, public_key: &str) -> Result<Vec<u8>> {
    let mut object = store
        .get(public_key)
        .await
        .map_err(|e| anyhow!("Failed to retrieve item from artifact store: {}", e))?;

    let mut result = Vec::new();
    object.read_to_end(&mut result).await?;    

    Ok(result)
}

#[cfg(test)]
mod test {
    use async_nats::jetstream;

    use super::{
        create_or_reuse_store, get_item_from_store, remove_item_from_store, write_to_store,
    };

    #[tokio::test]
    #[ignore]
    async fn test_round_trip() {
        const PK: &str = "Mxxxxx";
        const LATTICE: &str = "testlattice";
        const BUCKET: &str = "ARTIFACT_testlattice";

        let nc = async_nats::connect("0.0.0.0:4222").await.unwrap();
        let _ = create_or_reuse_store(nc.clone(), LATTICE).await.unwrap();
        let store = create_or_reuse_store(nc.clone(), LATTICE).await.unwrap();

        write_to_store(store.clone(), PK, vec![1, 2, 3, 4, 6])
            .await
            .unwrap();
        let resbytes = get_item_from_store(store.clone(), PK).await.unwrap();

        remove_item_from_store(store.clone(), PK).await.unwrap();

        let js = jetstream::new(nc);
        js.delete_object_store(BUCKET).await.unwrap();

        assert_eq!(vec![1, 2, 3, 4, 6], resbytes);
    }
}
