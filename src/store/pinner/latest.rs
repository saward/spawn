use super::Pinner;
use anyhow::Result;
use std::path::PathBuf;

/// Uses the latest versions of files, rather than any pinned version.
#[derive(Debug)]
pub struct Latest {
    folder: PathBuf,
}

/// Represents a snapshot of files and folders at a particular point in time.
/// Used to retrieve files as they were at that moment.
impl Latest {
    /// Folder represents the path to our history storage and latest files.
    /// If root is provided then store will use the files from archive rather
    /// than the latest live files.
    pub fn new(folder: PathBuf) -> Result<Self> {
        Ok(Self { folder })
    }
}

impl Pinner for Latest {
    /// Returns the file from the live file system if it exists.
    fn load(&self, name: &str) -> std::result::Result<Option<String>, minijinja::Error> {
        if let Ok(contents) = std::fs::read_to_string(self.folder.join(name)) {
            Ok(Some(contents))
        } else {
            Ok(None)
        }
    }
}
