use anyhow::{Context, Result};
use async_trait::async_trait;
use object_store::ObjectStore;
use object_store::PutPayload;
use serde::{Deserialize, Serialize};
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
pub(crate) fn read_hash_file(base_path: &str, hash: &str) -> Result<String> {
    let relative_path = hash_to_path(hash)?;
    let file_path = format!("{}/{}", base_path, relative_path);
    let contents = fs::read_to_string(file_path)?;

    Ok(contents)
}

/// Walks through a folder, creating pinned entries as appropriate for every
/// directory and file.  Returns a hash of the object.
pub(crate) async fn snapshot(
    object_store: &Box<dyn ObjectStore>,
    store_path: &Box<&str>,
    dir: &Box<&str>,
) -> Result<String> {
    // if dir.is_dir() {
    //     let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(Result::ok).collect();
    //     entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    //     let mut tree = Tree::default();

    //     for entry in entries {
    //         let path = &Box::new(entry.path().as_path());
    //         let name = path
    //             .file_name()
    //             .unwrap_or_default()
    //             .to_str()
    //             .unwrap_or_default();
    //         if path.is_dir() {
    //             let branch = snapshot(object_store, store_path, path).await?;
    //             tree.entries.push(Entry {
    //                 kind: EntryKind::Tree,
    //                 name: name.to_string(),
    //                 hash: branch,
    //             });
    //         } else {
    //             let hash = pin_file(object_store, store_path, path).await?;
    //             tree.entries.push(Entry {
    //                 kind: EntryKind::Blob,
    //                 name: name.to_string(),
    //                 hash,
    //             });
    //         }
    //     }

    //     let contents = toml::to_string(&tree).unwrap();
    //     let hash = pin_contents(object_store, store_path, contents).await?;

    //     return Ok(hash);
    // }
    Err(anyhow::anyhow!("store_path should be a folder"))
}

#[cfg(test)]
mod tests {
    use object_store::memory::InMemory;

    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_snapshot() -> Result<()> {
        // Simple test to ensure it runs without error.
        let source = "./static/example/components";
        let store_loc = "./test-store";

        let object_store: Box<dyn ObjectStore> = Box::new(InMemory::new());
        let root = snapshot(&object_store, &Box::from(store_loc), &Box::from(source)).await?;
        assert!(root.len() > 0);
        // Cleanup:
        fs::remove_dir_all(&store_loc)?;
        Ok(())
    }
}
