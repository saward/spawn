use super::Pinner;
use anyhow::anyhow;
use anyhow::Result;
use async_trait::async_trait;
use object_store::ObjectStore;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Spawn {
    files: Option<HashMap<String, String>>,
    store_path: String,
    source_path: String,
}

impl Spawn {
    pub fn new(store_path: &str, source_path: &str, root_hash: Option<&str>) -> Result<Self> {
        // Loop over our root and read into memory the entire tree for this root:
        let files = match root_hash {
            Some(hash) => {
                // let mut files = HashMap::new();
                // Self::read_root_hash(store_path, &mut files, "", hash)?;
                // Some(files)
                return Err(anyhow!("Not implemented"));
            }
            None => None,
        };

        let store = Self {
            files,
            store_path: store_path.to_string(),
            source_path: source_path.to_string(),
        };

        Ok(store)
    }

    fn read_root_hash(
        store_path: &PathBuf,
        files: &mut HashMap<String, PathBuf>,
        base_path: &Path,
        root_hash: &str,
    ) -> Result<()> {
        // let contents =
        //     super::read_hash_file(store_path, root_hash).context("cannot read root file")?;
        // let tree: super::Tree = toml::from_str(&contents).context("failed to parse tree TOML")?;

        // for entry in tree.entries {
        //     match entry.kind {
        //         super::EntryKind::Blob => {
        //             let full_name = format!("{}/{}", base_path.display(), &entry.name);
        //             let full_path = store_path.join(&super::hash_to_path(&entry.hash)?);
        //             files.insert(full_name, full_path);
        //         }
        //         super::EntryKind::Tree => {
        //             let new_base = base_path.join(&entry.name);
        //             Self::read_root_hash(store_path, files, &new_base, &entry.hash)?;
        //         }
        //     }
        // }

        // Ok(())
        Err(anyhow!("read_root_hash not implemented"))
    }
}

#[async_trait]
impl Pinner for Spawn {
    /// Returns the file from the store if it exists.
    async fn load(
        &self,
        name: &str,
        object_store: &Box<dyn ObjectStore>,
    ) -> Result<Option<String>> {
        // Borrow files from inside self.files, if not none:
        let files = self
            .files
            .as_ref()
            .ok_or(anyhow!("files not initialized, was a root hash specified?"))?;

        if let Some(path) = files.get(name) {
            if let Ok(contents) = std::fs::read_to_string(path) {
                Ok(Some(contents))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    async fn snapshot(&mut self, object_store: &Box<dyn ObjectStore>) -> Result<String> {
        super::snapshot(
            object_store,
            &Box::from(self.store_path.as_str()),
            &Box::from(self.source_path.as_str()),
        )
        .await
    }
}
