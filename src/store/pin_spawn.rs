use super::pin;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use twox_hash::xxhash3_128;

#[derive(Debug)]
pub struct PinStore {
    files: HashMap<String, PathBuf>,
    store_path: PathBuf,
}

impl PinStore {
    pub fn new(store_path: PathBuf, root: String) -> Result<Self> {
        // Loop over our root and read into memory the entire tree for this root:
        let files = HashMap::<String, PathBuf>::new();

        let mut store = Self { files, store_path };
        store.read_root(&PathBuf::new(), &root)?;

        Ok(store)
    }

    fn read_root(&mut self, base_path: &Path, root: &str) -> Result<()> {
        let contents =
            pin::read_hash_file(&self.store_path, root).context("cannot read root file")?;
        let tree: pin::Tree = toml::from_str(&contents).context("failed to parse tree TOML")?;

        for entry in tree.entries {
            match entry.kind {
                pin::EntryKind::Blob => {
                    let full_name = format!("{}/{}", base_path.display(), &entry.name);
                    let full_path = self.store_path.join(&pin::hash_to_path(&entry.hash)?);
                    self.files.insert(full_name, full_path);
                }
                pin::EntryKind::Tree => {
                    let new_base = base_path.join(&entry.name);
                    self.read_root(&new_base, &entry.hash)?;
                }
            }
        }

        Ok(())
    }
}

impl super::Store for PinStore {
    /// Returns the file from the store if it exists.
    fn load(&self, name: &str) -> std::result::Result<Option<String>, minijinja::Error> {
        if let Some(path) = self.files.get(name) {
            if let Ok(contents) = std::fs::read_to_string(path) {
                Ok(Some(contents))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }
}
