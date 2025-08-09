use anyhow::{Context, Result};
use async_trait::async_trait;

use object_store::ObjectStore;
use object_store::PutPayload;
use serde::{Deserialize, Serialize};

use futures::StreamExt;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
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
pub(crate) fn read_hash_file(base_path: &str, hash: &str) -> Result<String> {
    let relative_path = hash_to_path(hash)?;
    let file_path = format!("{}/{}", base_path, relative_path);
    let contents = fs::read_to_string(file_path)?;

    Ok(contents)
}

pub(crate) fn deprecated_pin_file(store_path: &Path, file_path: &Path) -> Result<String> {
    let contents = fs::read_to_string(file_path)?;

    deprecated_pin_contents(store_path, contents)
}

pub(crate) fn deprecated_pin_contents(store_path: &Path, contents: String) -> Result<String> {
    let hash = xxhash3_128::Hasher::oneshot(contents.as_bytes());
    let hash = format!("{:032x}", hash);
    let hash_folder = PathBuf::from(&hash[..2]);
    let dir = store_path.join(hash_folder.clone());
    let file = PathBuf::from(&hash[2..]);

    fs::create_dir_all(&dir).context(format!("could not create all dir at {:?}", &dir))?;
    let path = dir.join(file.clone());

    if !std::path::Path::new(&path).exists() {
        let mut f =
            fs::File::create(&path).context(format!("could not create file at {:?}", &path))?;
        f.write_all(contents.as_bytes())
            .context("could not write bytes")?;
    }

    Ok(hash)
}

/// Walks through a folder, creating pinned entries as appropriate for every
/// directory and file.  Returns a hash of the object.
pub(crate) fn deprecated_snapshot(store_path: &Path, dir: &Path) -> Result<String> {
    if dir.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(Result::ok).collect();
        entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

        let mut tree = Tree::default();

        for entry in entries {
            let path = entry.path();
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_str()
                .unwrap_or_default();
            if path.is_dir() {
                let branch = deprecated_snapshot(store_path, &path)?;
                tree.entries.push(Entry {
                    kind: EntryKind::Tree,
                    name: name.to_string(),
                    hash: branch,
                });
            } else {
                let hash = deprecated_pin_file(store_path, &path)?;
                tree.entries.push(Entry {
                    kind: EntryKind::Blob,
                    name: name.to_string(),
                    hash,
                });
            }
        }

        let contents = toml::to_string(&tree).unwrap();
        let hash = deprecated_pin_contents(store_path, contents)?;

        return Ok(hash);
    }
    Err(anyhow::anyhow!("store_path should be a folder"))
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
    use object_store::local::LocalFileSystem;
    // use object_store::memory::InMemory;

    use super::*;

    #[tokio::test]
    async fn test_snapshot() -> Result<()> {
        // Simple test to ensure it runs without error.
        let source = "/Users/mark/projects/saward/spawn/static/example";
        let store_loc = "/store";

        let object_store: Box<dyn ObjectStore> =
            Box::new(LocalFileSystem::new_with_prefix(&source)?);
        // let object_store: Box<dyn ObjectStore> = Box::new(InMemory::new());
        let root = snapshot(&object_store, store_loc, &format!("/{}", "components")).await?;
        assert!(root.len() > 0);
        Ok(())
    }
}
