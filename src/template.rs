use crate::pinfile::LockData;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Result;

/// A custom template loader that loads templates on demand and tracks which ones were loaded
#[derive(Debug)]
pub struct ComponentLoader {
    components_path: PathBuf,
    pinned_path: PathBuf,
    lock_data: Option<LockData>,
    loaded_files: Mutex<HashMap<String, String>>,
}

impl ComponentLoader {
    pub fn new(
        components_path: PathBuf,
        pinned_path: PathBuf,
        lock_data: Option<LockData>,
    ) -> Self {
        Self {
            components_path,
            pinned_path,
            loaded_files: Mutex::new(HashMap::new()),
            lock_data,
        }
    }

    pub fn get_loaded_files(&self) -> HashMap<String, String> {
        self.loaded_files.lock().unwrap().clone()
    }

    pub fn load(&self, name: &str) -> Result<Option<String>, minijinja::Error> {
        let file_path = match &self.lock_data {
            Some(lock_data) => {
                let hash = &lock_data.entries.get(name).unwrap().hash;
                self.pinned_path.join(&hash[..2]).join(&hash[2..])
            }
            None => self.components_path.join(name),
        };
        if let Ok(contents) = std::fs::read_to_string(&file_path) {
            // Track that we loaded this file
            self.loaded_files
                .lock()
                .unwrap()
                .insert(name.to_string(), contents.clone());
            Ok(Some(contents))
        } else {
            Ok(None)
        }
    }
}
