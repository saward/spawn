use super::Pinner;
use anyhow::Result;
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use opendal::Operator;
use std::collections::HashMap;

#[derive(Debug)]
pub struct Spawn {
    files: Option<HashMap<String, String>>,
    store_path: String,
    source_path: String,
}

impl Spawn {
    pub fn new(store_path: &str, source_path: &str) -> Result<Self> {
        let store = Self {
            files: None,
            store_path: store_path.to_string(),
            source_path: source_path.to_string(),
        };

        Ok(store)
    }

    pub async fn new_with_root_hash(
        store_path: &str,
        source_path: &str,
        root_hash: &str,
        object_store: &Operator,
    ) -> Result<Self> {
        let mut files = HashMap::new();
        Self::read_root_hash(object_store, store_path, &mut files, "", root_hash).await?;

        let store = Self {
            files: Some(files),
            store_path: store_path.to_string(),
            source_path: source_path.to_string(),
        };

        Ok(store)
    }

    async fn read_root_hash(
        object_store: &Operator,
        store_path: &str,
        files: &mut HashMap<String, String>,
        base_path: &str,
        root_hash: &str,
    ) -> Result<()> {
        let contents = super::read_hash_file(object_store, store_path, root_hash)
            .await
            .context("cannot read root file")?;
        let tree: super::Tree = toml::from_str(&contents).context("failed to parse tree TOML")?;

        for entry in tree.entries {
            match entry.kind {
                super::EntryKind::Blob => {
                    let full_name = if base_path.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{}/{}", base_path, &entry.name)
                    };
                    let full_path = format!("{}/{}", store_path, super::hash_to_path(&entry.hash)?);
                    files.insert(full_name, full_path);
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
    async fn load(&self, name: &str, object_store: &Operator) -> Result<Option<String>> {
        // Borrow files from inside self.files, if not none:
        let files = self
            .files
            .as_ref()
            .ok_or(anyhow!("files not initialized, was a root hash specified?"))?;

        if let Some(path) = files.get(name) {
            match object_store.read(path).await {
                Ok(get_result) => {
                    let bytes = get_result.to_bytes();
                    let contents = String::from_utf8(bytes.to_vec())?;
                    Ok(Some(contents))
                }
                Err(_) => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    async fn snapshot(&mut self, object_store: &Operator) -> Result<String> {
        super::snapshot(object_store, &self.store_path, &self.components_folder()).await
    }
}
