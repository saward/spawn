use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use twox_hash::xxhash3_128;

#[derive(Clone, Debug)]
enum Node {
    File { detail: NodeDetail, hash: String },
    Folder { detail: NodeDetail, tree: Vec<Node> },
}

impl Node {
    fn print_node(&self, depth: u16) {
        match self {
            Node::File { detail, hash } => {
                println!("{} {:?}", "-".repeat(depth as usize), detail.name);
            }
            Node::Folder { detail, tree } => {
                println!("{} {:?}", "-".repeat(depth as usize), detail.name);
                for node in tree {
                    node.print_node(depth + 1);
                }
            }
        };
    }

    // /// Store will write this node's contents or tree to a file and then return
    // /// a reference to its hash.
    // fn store(&self) -> Result<String> {}
}

#[derive(Clone, Debug)]
pub struct NodeDetail {
    name: String,  // TODO: Symbolic filesystem location (e.g., "./components/blah.sql")
    path: PathBuf, // Real filesystem location, which could be archive (e.g., "ab/cdefghijklmnop")
}

/// Stores the
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
pub struct ArchiveStore {
    root: PathBuf,
}

impl ArchiveStore {
    pub fn new(folder: PathBuf, commit: String) -> Result<Self> {
        Err(anyhow::anyhow!("not implemented"))
    }
}

fn snapshot(store_path: &Path, dir: &Path) -> Result<String> {
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

// fn visit_dirs(dir: &Path) -> Result<Node> {
//     if dir.is_dir() {
//         let tree_detail = NodeDetail {
//             name: dir
//                 .file_name()
//                 .unwrap_or_default()
//                 .to_str()
//                 .unwrap_or_default()
//                 .to_string(),
//             path: dir.to_path_buf(),
//         };
//         let mut tree: Vec<Node> = Vec::new();
//         let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(Result::ok).collect();
//         entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
//
//         for entry in entries {
//             let path = entry.path();
//             if path.is_dir() {
//                 println!("* {:?}", entry.path());
//                 let branch = visit_dirs(&path)?;
//                 tree.push(branch);
//             } else {
//                 let detail = NodeDetail {
//                     name: path.to_str().unwrap_or_default().to_string(),
//                     path: path.to_path_buf(),
//                 };
//
//                 tree.push(Node::File {
//                     detail,
//                     hash: "Fake hash".to_string(),
//                 });
//                 println!("* {:?}", entry.path());
//             }
//         }
//
//         let root = Node::Folder {
//             detail: tree_detail,
//             tree,
//         };
//         return Ok(root);
//     }
//     Err(anyhow::anyhow!("wtf this isn't a folder?!?"))
// }

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

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use std::path::PathBuf;
//
//     #[test]
//     fn test_store_new() {
//         let path = PathBuf::from("./static/example");
//         let store = Store::new(path.clone()).unwrap();
//
//         store.tree.print_node(0);
//         assert_eq!(store.folder, path);
//     }
// }
