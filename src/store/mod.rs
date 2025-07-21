use anyhow::Result;
use std::path::PathBuf;

pub mod local_fs;
pub mod pin;
pub mod pin_spawn;

pub trait Store {
    fn load(&self, name: &str) -> std::result::Result<Option<String>, minijinja::Error>;
}

#[derive(Debug)]
pub struct LiveStore {
    folder: PathBuf,
}

/// Represents a snapshot of files and folders at a particular point in time.
/// Used to retrieve files as they were at that moment.
impl LiveStore {
    /// Folder represents the path to our history storage and current files.
    /// If root is provided then store will use the files from archive rather
    /// than the current live files.
    pub fn new(folder: PathBuf) -> Result<Self> {
        Ok(Self { folder })
    }
}

impl Store for LiveStore {
    /// Returns the file from the live file system if it exists.
    fn load(&self, name: &str) -> std::result::Result<Option<String>, minijinja::Error> {
        if let Ok(contents) = std::fs::read_to_string(self.folder.join(name)) {
            Ok(Some(contents))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pin;
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn test_snapshot() -> Result<()> {
        // Simple test to ensure it runs without error.
        let source = PathBuf::from("./static/example/components");
        let store_loc = PathBuf::from("./test-store");
        let root = pin::snapshot(&store_loc, &source)?;
        assert!(root.len() > 0);
        // Cleanup:
        fs::remove_dir_all(&store_loc)?;
        Ok(())
    }
}
