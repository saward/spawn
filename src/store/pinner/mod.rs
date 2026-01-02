use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::TryStreamExt;
use opendal::Operator;

use serde::{Deserialize, Serialize};

use futures::StreamExt;
use std::fmt::Debug;
use twox_hash::xxhash3_128;

pub mod latest;
pub mod spawn;

#[async_trait]
pub trait Pinner: Debug + Send + Sync {
    async fn load(&self, name: &str, fs: &Operator) -> Result<Option<String>>;
    async fn snapshot(&mut self, fs: &Operator) -> Result<String>;

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

pub(crate) async fn pin_contents(
    fs: &Operator,
    store_path: &str,
    contents: String,
) -> Result<String> {
    let hash = xxhash3_128::Hasher::oneshot(contents.as_bytes());
    let hash = format!("{:032x}", hash);
    let dir = format!("{}/{}", store_path, hash_to_path(&hash)?);

    fs.write(&dir, contents).await?;

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
pub(crate) async fn read_hash_file(fs: &Operator, base_path: &str, hash: &str) -> Result<String> {
    let relative_path = hash_to_path(hash)?;
    let file_path = format!("{}/{}", base_path, relative_path);

    let get_result = fs.read(&file_path).await?;
    let bytes = get_result.to_bytes();
    let contents = String::from_utf8(bytes.to_vec())?;

    Ok(contents)
}

/// Walks through objects in an ObjectStore, creating pinned entries as appropriate for every
/// directory and file.  Returns a hash of the object.
pub(crate) async fn snapshot(fs: &Operator, store_path: &str, prefix: &str) -> Result<String> {
    // list_with_delimiter seems to return only immediate children objects and
    // folders, rather than every subfolder.  It behaves more like a directory
    // walk than a full list of all nested folders and objects like list.
    let mut fs_lister = fs.lister(prefix).await?;
    let mut list_result: Vec<opendal::Entry> = Vec::new();
    while let Some(entry) = fs_lister.try_next().await? {
        if entry.path() == prefix {
            continue;
        }
        list_result.push(entry);
    }

    let mut tree = Tree::default();
    let mut entries = Vec::new();

    for entry in list_result {
        match entry.path().ends_with("/") {
            true => {
                // Folder
                println!("new prefix: {}, current prefix: {}", entry.path(), prefix);
                let branch = Box::pin(snapshot(fs, store_path, entry.path()))
                    .await
                    .context("failed to snapshot subfolder")?;
                entries.push((
                    entry.name().to_string(),
                    Entry {
                        kind: EntryKind::Tree,
                        name: entry
                            .name()
                            .strip_suffix("/")
                            .unwrap_or(entry.name())
                            .to_string(),
                        hash: branch,
                    },
                ));
            }
            false => {
                // File
                // Stream-based hashing to avoid loading large files into memory
                let object_result = fs
                    .reader(entry.path())
                    .await
                    .context("could not get reader for file")?;

                println!("Snapshotting {}", entry.path());

                let mut reader = object_result.into_stream(0..).await?;
                let mut hasher = xxhash3_128::Hasher::new();

                while let Some(chunk) = reader.next().await {
                    let chunk = chunk.context("failed to read chunk from object stream")?;
                    hasher.write(&chunk.to_bytes());
                }

                let hash = format!("{:032x}", hasher.finish_128());

                entries.push((
                    entry.name().to_string(),
                    Entry {
                        kind: EntryKind::Blob,
                        name: entry.name().to_string(),
                        hash,
                    },
                ));
            }
        }
    }

    // Sort entries by name for consistent ordering, and then return a hash for
    // this node.
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    tree.entries = entries.into_iter().map(|(_, entry)| entry).collect();

    let contents = toml::to_string(&tree).unwrap();
    let hash = pin_contents(fs, store_path, contents)
        .await
        .context("could not pin contents")?;

    Ok(hash)
}

#[cfg(test)]
mod tests {
    use opendal::services::Memory;

    use super::*;

    #[cfg(test)]
    async fn populate_store_from_store(
        source_store: &Operator,
        target_store: &Operator,
        prefix: &str,
    ) -> Result<()> {
        let mut lister = source_store
            .lister_with(prefix)
            .recursive(true)
            .await
            .context("lister call")?;
        let mut list_result: Vec<opendal::Entry> = Vec::new();

        println!("Trying to write all");
        while let Some(entry) = lister.try_next().await? {
            println!("found {}", entry.path());
            if entry.path().ends_with("/") {
                continue;
            }
            list_result.push(entry);
        }

        for entry in list_result {
            // Print out the file we're writing:
            println!("Writing {}", entry.path());
            let object_path = entry.path();

            // Get the object data
            let bytes = source_store
                .read(object_path)
                .await
                .context(format!("read path {}", &object_path))?;

            // Store in target with the same path
            target_store
                .write(object_path, bytes)
                .await
                .context("write")?;
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_snapshot() -> Result<()> {
        let store_loc = "store/";

        let dest_service = Memory::default();
        let dest_op = Operator::new(dest_service)?.finish();

        // Useful for writing to local file system for testing/debugging:
        // let dest_service = opendal::services::Fs::default().root("./testout");
        // let dest_op = Operator::new(dest_service)?.finish();

        // Create a LocalFileSystem to read from static/example
        let fs_service = opendal::services::Fs::default().root("./static/example");
        let source_store = Operator::new(fs_service).context("new operator")?.finish();

        // Populate the in-memory store with contents from static/example
        populate_store_from_store(&source_store, &dest_op, "")
            .await
            .context("call to populate memory fs from object store")?;

        let root = snapshot(&dest_op, store_loc, "components/").await?;
        let mut lister = dest_op
            .lister_with("store/")
            .recursive(true)
            .await
            .context("lister call")?;

        while let Some(entry) = lister.try_next().await? {
            println!("in in memory found {}", entry.path());
        }
        assert!(root.len() > 0);
        assert_eq!("cb59728fefa959672ef3c8c9f0b6df95", root);

        // Read and print the root level file
        let root_content = read_hash_file(&dest_op, store_loc, &root).await?;

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
