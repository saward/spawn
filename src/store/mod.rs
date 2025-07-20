use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use twox_hash::xxhash3_128;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Tree {
    entries: Vec<Entry>,
}

#[derive(Debug, Deserialize, Serialize)]
enum EntryKind {
    Blob,
    Tree,
}

#[derive(Debug, Deserialize, Serialize)]
struct Entry {
    kind: EntryKind,
    hash: String,
    name: String,
}

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
        let contents = read_hash_file(&self.store_path, root).context("cannot read root file")?;
        let tree: Tree = toml::from_str(&contents).context("failed to parse tree TOML")?;

        for entry in tree.entries {
            match entry.kind {
                EntryKind::Blob => {
                    let full_name = format!("{}/{}", base_path.display(), &entry.name);
                    let full_path = self.store_path.join(&hash_to_path(&entry.hash)?);
                    self.files.insert(full_name, full_path);
                }
                EntryKind::Tree => {
                    let new_base = base_path.join(&entry.name);
                    self.read_root(&new_base, &entry.hash)?;
                }
            }
        }

        Ok(())
    }
}

impl Store for PinStore {
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

/// Converts a hash string into a relative path like `c6/b8e869fa533155bbf2f0dd8fda9c68`.
fn hash_to_path(hash: &str) -> Result<PathBuf> {
    if hash.len() < 3 {
        return Err(anyhow::anyhow!("Hash too short"));
    }

    let (first_two, rest) = hash.split_at(2);
    Ok(PathBuf::from(first_two).join(rest))
}

/// Reads the file corresponding to the hash from the given base path.
fn read_hash_file(base_path: &Path, hash: &str) -> Result<String> {
    let relative_path = hash_to_path(hash)?;
    let file_path = base_path.join(relative_path);
    let contents = fs::read_to_string(file_path)?;

    Ok(contents)
}

/// Walks through a folder, creating pinned entries as appropriate for every
/// directory and file.  Returns a hash of the object.
pub fn snapshot(store_path: &Path, dir: &Path) -> Result<String> {
    if dir.is_dir() {
        let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(Result::ok).collect();
        entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

        let mut tree = Tree::default();

        for entry in entries {
            let path = entry.path();
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_str()
                .unwrap_or_default();
            if path.is_dir() {
                let branch = snapshot(store_path, &path)?;
                tree.entries.push(Entry {
                    kind: EntryKind::Tree,
                    name: name.to_string(),
                    hash: branch,
                });
            } else {
                let hash = pin_file(store_path, &path)?;
                tree.entries.push(Entry {
                    kind: EntryKind::Blob,
                    name: name.to_string(),
                    hash,
                });
            }
        }

        let contents = toml::to_string(&tree).unwrap();
        let hash = pin_contents(store_path, contents)?;

        return Ok(hash);
    }
    Err(anyhow::anyhow!("store_path should be a folder"))
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
