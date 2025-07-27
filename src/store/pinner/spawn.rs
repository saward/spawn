use super::Pinner;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug)]
pub struct Spawn {
    files: HashMap<String, PathBuf>,
    store_path: PathBuf,
}

impl Spawn {
    pub fn new(store_path: PathBuf, root_hash: String) -> Result<Self> {
        // Loop over our root and read into memory the entire tree for this root:
        let files = HashMap::<String, PathBuf>::new();

        let mut store = Self { files, store_path };
        store.read_root_hash(&PathBuf::new(), &root_hash)?;

        Ok(store)
    }

    fn read_root_hash(&mut self, base_path: &Path, root_hash: &str) -> Result<()> {
        let contents =
            super::read_hash_file(&self.store_path, root_hash).context("cannot read root file")?;
        let tree: super::Tree = toml::from_str(&contents).context("failed to parse tree TOML")?;

        for entry in tree.entries {
            match entry.kind {
                super::EntryKind::Blob => {
                    let full_name = format!("{}/{}", base_path.display(), &entry.name);
                    let full_path = self.store_path.join(&super::hash_to_path(&entry.hash)?);
                    self.files.insert(full_name, full_path);
                }
                super::EntryKind::Tree => {
                    let new_base = base_path.join(&entry.name);
                    self.read_root_hash(&new_base, &entry.hash)?;
                }
            }
        }

        Ok(())
    }
}

impl Pinner for Spawn {
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
