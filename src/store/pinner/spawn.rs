use super::Pinner;
use anyhow::Result;
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use opendal::Operator;
use std::collections::HashMap;

#[derive(Debug)]
pub struct Spawn {
    files: Option<HashMap<String, PinnedFile>>,
    pin_path: String,
    source_path: String,
}

/// Where a pinned file's contents live, and the hash its path is supposed to
/// correspond to — kept alongside the path so a load can verify the two
/// still agree.
#[derive(Debug, Clone)]
struct PinnedFile {
    path: String,
    hash: String,
}

impl Spawn {
    pub fn new(pin_path: String, source_path: String) -> Result<Self> {
        let store = Self {
            files: None,
            pin_path,
            source_path,
        };

        Ok(store)
    }

    pub async fn new_with_root_hash(
        pin_path: String,
        source_path: String,
        root_hash: &str,
        object_store: &Operator,
    ) -> Result<Self> {
        let mut files = HashMap::new();
        Self::read_root_hash(object_store, &pin_path, &mut files, "", root_hash).await?;

        let store = Self {
            files: Some(files),
            pin_path: pin_path.clone(),
            source_path,
        };

        Ok(store)
    }

    async fn read_root_hash(
        object_store: &Operator,
        store_path: &str,
        files: &mut HashMap<String, PinnedFile>,
        base_path: &str,
        root_hash: &str,
    ) -> Result<()> {
        let contents = super::read_hash_file(object_store, store_path, root_hash)
            .await
            .context("cannot read root file")?;
        let tree: super::Tree = toml::from_str(&contents).context("failed to parse tree TOML")?;

        for (_, entry) in tree.entries.iter().enumerate() {
            match entry.kind {
                super::EntryKind::Blob => {
                    let full_name = if base_path.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{}/{}", base_path, &entry.name)
                    };
                    let full_path = format!("{}/{}", store_path, super::hash_to_path(&entry.hash)?);
                    files.insert(
                        full_name,
                        PinnedFile {
                            path: full_path,
                            hash: entry.hash.clone(),
                        },
                    );
                }
                super::EntryKind::Tree => {
                    let new_base = if base_path.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{}/{}", base_path, &entry.name)
                    };
                    Box::pin(Self::read_root_hash(
                        object_store,
                        store_path,
                        files,
                        &new_base,
                        &entry.hash,
                    ))
                    .await?;
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
impl Pinner for Spawn {
    /// Returns the file from the store if it exists.
    async fn load_bytes(&self, name: &str, object_store: &Operator) -> Result<Option<Vec<u8>>> {
        // Borrow files from inside self.files, if not none:
        let files = self
            .files
            .as_ref()
            .ok_or(anyhow!("files not initialized, was a root hash specified?"))?;

        if let Some(pinned) = files.get(name) {
            match object_store.read(&pinned.path).await {
                Ok(get_result) => {
                    let bytes = get_result.to_bytes().to_vec();
                    super::verify_hash(&bytes, &pinned.hash, &pinned.path)?;
                    Ok(Some(bytes))
                }
                Err(_) => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    async fn snapshot(&mut self, object_store: &Operator) -> Result<String> {
        super::snapshot(object_store, Some(self.pin_path.as_str()), &self.source_path).await
    }
}
