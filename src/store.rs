use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use twox_hash::xxhash3_128;

fn pin_file(store_path: &Path, file_path: &Path) -> Result<String> {
    let contents = fs::read_to_string(file_path)?;

    pin_contents(store_path, contents)
}

fn pin_contents(store_path: &Path, contents: String) -> Result<String> {
    let hash = xxhash3_128::Hasher::oneshot(contents.as_bytes());
    let hash = format!("{:032x}", hash);
    let hash_folder = PathBuf::from(&hash[..2]);
    let dir = store_path.join(hash_folder.clone());
    let file = PathBuf::from(&hash[2..]);

    fs::create_dir_all(&dir).context(format!("could not create all dir at {:?}", &dir))?;
    let path = dir.join(file.clone());

    if !std::path::Path::new(&path).exists() {
        let mut f =
            fs::File::create(&path).context(format!("could not create file at {:?}", &path))?;
        f.write_all(contents.as_bytes())
            .context("could not write bytes")?;
    }

    Ok(hash)
}

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

#[derive(Debug)]
pub struct PinStore {
    store_path: PathBuf,
    root: String,
}

impl PinStore {
    pub fn new(store_path: PathBuf, root: String) -> Self {
        Self { store_path, root }
    }
}

impl Store for PinStore {
    /// Returns the file from the live file system if it exists.
    fn load(&self, name: &str) -> std::result::Result<Option<String>, minijinja::Error> {
        Ok(None)
        // if let Ok(contents) = std::fs::read_to_string(self.folder.join(name)) {
        //     Ok(Some(contents))
        // } else {
        //     Ok(None)
        // }
    }
}

pub fn snapshot(store_path: &Path, dir: &Path) -> Result<String> {
    if dir.is_dir() {
        let mut tree = String::new();
        let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(Result::ok).collect();
        entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                let branch = snapshot(store_path, &path)?;
                tree.push_str(&format!(
                    "tree\t{}\t{}\n",
                    branch,
                    path.file_name()
                        .unwrap_or_default()
                        .to_str()
                        .unwrap_or_default()
                ));
            } else {
                let hash = pin_file(store_path, &path)?;
                tree.push_str(&format!(
                    "blob\t{}\t{}\n",
                    hash,
                    path.file_name()
                        .unwrap_or_default()
                        .to_str()
                        .unwrap_or_default()
                ));
            }
        }

        let hash = pin_contents(store_path, tree)?;

        return Ok(hash);
    }
    Err(anyhow::anyhow!("wtf this isn't a folder?!?"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_snapshot() -> Result<()> {
        // Simple test to ensure it runs without error.
        let source = PathBuf::from("./static/example/components");
        let store_loc = PathBuf::from("./test-store");
        let root = snapshot(&store_loc, &source)?;
        assert!(root.len() > 0);
        // Cleanup:
        fs::remove_dir_all(&store_loc)?;
        Ok(())
    }
}
