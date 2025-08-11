use anyhow::{Context, Result};
use async_trait::async_trait;

use object_store::ObjectStore;
use object_store::PutPayload;
use serde::{Deserialize, Serialize};

use futures::StreamExt;
use std::fs;
use twox_hash::xxhash3_128;

pub mod latest;
pub mod spawn;

#[async_trait]
pub trait Pinner: Send + Sync {
    async fn load(&self, name: &str, object_store: &Box<dyn ObjectStore>)
        -> Result<Option<String>>;
    async fn snapshot(&mut self, object_store: &Box<dyn ObjectStore>) -> Result<String>;

    fn components_folder(&self) -> &'static str {
        "components"
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Tree {
    pub entries: Vec<Entry>,
}

#[derive(Debug, Deserialize, Serialize)]
pub enum EntryKind {
    Blob,
    Tree,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Entry {
    pub kind: EntryKind,
    pub hash: String,
    pub name: String,
}

pub(crate) async fn pin_file(
    object_store: &Box<dyn ObjectStore>,
    store_path: &str,
    file_path: &str,
) -> Result<String> {
    let contents = fs::read_to_string(file_path)?;

    pin_contents(object_store, store_path, contents).await
}

pub(crate) async fn pin_contents(
    object_store: &Box<dyn ObjectStore>,
    store_path: &str,
    contents: String,
) -> Result<String> {
    let hash = xxhash3_128::Hasher::oneshot(contents.as_bytes());
    let hash = format!("{:032x}", hash);
    let dir = format!("{}/{}", store_path, hash_to_path(&hash)?);

    let payload: PutPayload = contents.into();
    object_store.put(&dir.into(), payload).await?;

    // fs::create_dir_all(&dir).context(format!("could not create all dir at {:?}", &dir))?;
    // let path = dir.join(file.clone());

    // if !std::path::Path::new(&path).exists() {
    //     let mut f =
    //         fs::File::create(&path).context(format!("could not create file at {:?}", &path))?;
    //     f.write_all(contents.as_bytes())
    //         .context("could not write bytes")?;
    // }

    Ok(hash)
}

/// Converts a hash string into a relative path like `c6/b8e869fa533155bbf2f0dd8fda9c68`.
pub(crate) fn hash_to_path(hash: &str) -> Result<String> {
    if hash.len() < 3 {
        return Err(anyhow::anyhow!("Hash too short"));
    }

    let (first_two, rest) = hash.split_at(2);
    Ok(format!("{}/{}", first_two, rest).to_string())
}

/// Reads the file corresponding to the hash from the given base path.
pub(crate) async fn read_hash_file(
    object_store: &Box<dyn ObjectStore>,
    base_path: &str,
    hash: &str,
) -> Result<String> {
    let relative_path = hash_to_path(hash)?;
    let file_path = format!("{}/{}", base_path, relative_path);

    let get_result = object_store.get(&file_path.into()).await?;
    let bytes = get_result.bytes().await?;
    let contents = String::from_utf8(bytes.to_vec())?;

    Ok(contents)
}

/// Walks through objects in an ObjectStore, creating pinned entries as appropriate for every
/// directory and file.  Returns a hash of the object.
pub(crate) async fn snapshot(
    object_store: &Box<dyn ObjectStore>,
    store_path: &str,
    prefix: &str,
) -> Result<String> {
    // Convert prefix to ObjectStore Path
    let prefix_path = if prefix.is_empty() {
        None
    } else {
        Some(object_store::path::Path::from(prefix))
    };

    // list_with_delimiter seems to return only immediate children objects and
    // folders, rather than every subfolder.  It behaves more like a directory
    // walk than a full list of all nested folders and objects like list.
    let mut list_result = object_store
        .list_with_delimiter(prefix_path.as_ref())
        .await
        .context("could not list object store")?;

    let mut tree = Tree::default();
    let mut entries = Vec::new();

    // snapshot runs recursively.  prefix tells us the full path of the current
    // folder (common_prefix) we are processing.  First, we find all subfolders
    // of that prefix, call snapshot for them to get the hashes, and store those
    // Tree entries with their hashes.  We do this by stripping the current
    // snapshot call's prefix, so we're left only with subfolders of this tree.
    list_result.common_prefixes.sort();
    for common_prefix in list_result.common_prefixes {
        let dir_name = common_prefix
            .as_ref()
            .strip_prefix(&format!("{}/", prefix))
            .unwrap_or(common_prefix.as_ref())
            .trim_end_matches('/');

        if !dir_name.is_empty() {
            let branch = Box::pin(snapshot(object_store, store_path, common_prefix.as_ref()))
                .await
                .context("failed to snapshot subfolder")?;
            entries.push((
                dir_name.to_string(),
                Entry {
                    kind: EntryKind::Tree,
                    name: dir_name.to_string(),
                    hash: branch,
                },
            ));
        }
    }

    // Now that all the directories have been hashes for this prefix, we can do
    // the same for objects, finding those with the prefix we care about.  Look
    // through all objects, and find the ones that have a prefix matching our
    // current prefix.  Then, we add a blob entry with hash for each of those:
    for object_meta in list_result.objects {
        let full_path = object_meta.location.as_ref();

        let file_name = if prefix.is_empty() {
            full_path
        } else {
            full_path
                .strip_prefix(&format!("{}/", prefix))
                .unwrap_or(full_path)
        };

        // Skip if this is not a direct child (contains additional slashes)
        if !file_name.contains('/') && !file_name.is_empty() {
            // Stream-based hashing to avoid loading large files into memory
            let object_result = object_store
                .get(&object_meta.location)
                .await
                .context("could not get object to hash")?;

            let mut reader = object_result.into_stream();
            let mut hasher = xxhash3_128::Hasher::new();

            while let Some(chunk) = reader.next().await {
                let chunk = chunk.context("failed to read chunk from object stream")?;
                hasher.write(&chunk);
            }

            let hash = format!("{:032x}", hasher.finish_128());

            entries.push((
                file_name.to_string(),
                Entry {
                    kind: EntryKind::Blob,
                    name: file_name.to_string(),
                    hash,
                },
            ));
        }
    }

    // Sort entries by name for consistent ordering, and then return a hash for
    // this node.
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    tree.entries = entries.into_iter().map(|(_, entry)| entry).collect();

    let contents = toml::to_string(&tree).unwrap();
    let hash = pin_contents(object_store, store_path, contents)
        .await
        .context("could not pin contents")?;

    Ok(hash)
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use object_store::local::LocalFileSystem;
    use object_store::memory::InMemory;

    use super::*;

    async fn populate_inmemory_from_object_store(
        source_store: &Box<dyn ObjectStore>,
        target_store: &InMemory,
        prefix: &str,
    ) -> Result<()> {
        let mut stream = source_store.list(Some(&prefix.into()));

        while let Some(meta) = stream.next().await {
            let meta = meta?;
            let object_path = &meta.location;

            // Get the object data
            let get_result = source_store.get(object_path).await?;
            let bytes = get_result.bytes().await?;

            // Store in target with the same path
            target_store.put(object_path, bytes.into()).await?;
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_snapshot() -> Result<()> {
        // Simple test to ensure it runs without error.
        let store_loc = "/store";

        let inmemory = InMemory::new();

        // Create a LocalFileSystem to read from static/example
        let source_store: Box<dyn ObjectStore> =
            Box::new(LocalFileSystem::new_with_prefix("./static/example")?);

        // Populate the in-memory store with contents from static/example
        populate_inmemory_from_object_store(&source_store, &inmemory, "").await?;

        let object_store: Box<dyn ObjectStore> = Box::new(inmemory);
        let root = snapshot(&object_store, store_loc, "components").await?;
        assert!(root.len() > 0);
        assert_eq!("cb59728fefa959672ef3c8c9f0b6df95", root);

        // Read and print the root level file
        let root_content = read_hash_file(&object_store, store_loc, &root).await?;

        // Verify that the hash of the root content matches the snapshot hash
        let content_hash = format!(
            "{:032x}",
            xxhash3_128::Hasher::oneshot(root_content.as_bytes())
        );
        assert_eq!(
            root, content_hash,
            "Snapshot hash should match content hash"
        );

        Ok(())
    }
}
