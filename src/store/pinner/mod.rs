use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::TryStreamExt;
use opendal::Operator;

use serde::{Deserialize, Serialize};

use std::fmt::Debug;
use twox_hash::xxhash3_128;

pub mod latest;
pub mod spawn;

#[async_trait]
pub trait Pinner: Debug + Send + Sync {
    async fn load_bytes(&self, name: &str, fs: &Operator) -> Result<Option<Vec<u8>>>;

    async fn load(&self, name: &str, fs: &Operator) -> Result<Option<String>> {
        match self.load_bytes(name, fs).await? {
            Some(bytes) => Ok(Some(String::from_utf8(bytes)?)),
            None => Ok(None),
        }
    }

    async fn snapshot(&mut self, fs: &Operator) -> Result<String>;
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
pub(crate) async fn snapshot(fs: &Operator, store_path: &str, mut prefix: &str) -> Result<String> {
    let fixed;
    if !prefix.ends_with("/") {
        fixed = format!("{}/", prefix);
        prefix = &fixed;
    }

    let mut fs_lister = fs.lister(prefix).await?;
    let mut list_result: Vec<opendal::Entry> = Vec::new();
    while let Some(entry) = fs_lister.try_next().await? {
        if entry.path() == prefix {
            continue;
        }
        list_result.push(entry);
    }

    let mut entries = Vec::new();

    for entry in list_result {
        match entry.path().ends_with("/") {
            true => {
                // Folder
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
                let contents = fs.read(entry.path()).await?;
                let contents = String::from_utf8(contents.to_bytes().to_vec())?;
                let hash = pin_contents(fs, store_path, contents).await?;

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
    let mut tree = Tree::default();
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
    use crate::store;

    use super::*;

    #[tokio::test]
    async fn test_snapshot() -> Result<()> {
        let dest_op =
            store::disk_to_operator("./static/example", None, store::DesiredOperator::Memory)
                .await?;

        let store_loc = "store/";
        let root = snapshot(&dest_op, store_loc, "components/").await?;

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
