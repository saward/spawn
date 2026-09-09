use super::Pinner;
use anyhow::Result;
use anyhow::{anyhow, Context};
use async_trait::async_trait;
use opendal::Operator;
use std::collections::HashMap;

use crate::config::FolderPather;
use crate::hash::content_hash;
use crate::pinfile;
use crate::store::list_migration_fs_status;

#[derive(Debug)]
pub struct Spawn {
    files: Option<HashMap<String, PinnedFile>>,
    pin_path: String,
    source_path: String,
}

/// Where a pinned file's contents live, and the hash its path is supposed to
/// correspond to — kept alongside the path so a load can verify the two
/// still agree.
#[derive(Debug, Clone)]
struct PinnedFile {
    path: String,
    hash: String,
}

impl Spawn {
    pub fn new(pin_path: String, source_path: String) -> Result<Self> {
        let store = Self {
            files: None,
            pin_path,
            source_path,
        };

        Ok(store)
    }

    pub async fn new_with_root_hash(
        pin_path: String,
        source_path: String,
        root_hash: &str,
        object_store: &Operator,
    ) -> Result<Self> {
        let mut files = HashMap::new();
        Self::read_root_hash(object_store, &pin_path, &mut files, "", root_hash).await?;

        let store = Self {
            files: Some(files),
            pin_path: pin_path.clone(),
            source_path,
        };

        Ok(store)
    }

    async fn read_root_hash(
        object_store: &Operator,
        store_path: &str,
        files: &mut HashMap<String, PinnedFile>,
        base_path: &str,
        root_hash: &str,
    ) -> Result<()> {
        let contents = super::read_hash_file(object_store, store_path, root_hash)
            .await
            .context("cannot read root file")?;
        let tree: super::Tree = toml::from_str(&contents).context("failed to parse tree TOML")?;

        for (_, entry) in tree.entries.iter().enumerate() {
            match entry.kind {
                super::EntryKind::Blob => {
                    let full_name = if base_path.is_empty() {
                        entry.name.clone()
                    } else {
                        format!("{}/{}", base_path, &entry.name)
                    };
                    let full_path = format!("{}/{}", store_path, super::hash_to_path(&entry.hash)?);
                    files.insert(
                        full_name,
                        PinnedFile {
                            path: full_path,
                            hash: entry.hash.clone(),
                        },
                    );
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
    async fn load_bytes(&self, name: &str, object_store: &Operator) -> Result<Option<Vec<u8>>> {
        // Borrow files from inside self.files, if not none:
        let files = self
            .files
            .as_ref()
            .ok_or(anyhow!("files not initialized, was a root hash specified?"))?;

        if let Some(pinned) = files.get(name) {
            match object_store.read(&pinned.path).await {
                Ok(get_result) => {
                    let bytes = get_result.to_bytes().to_vec();
                    super::verify_hash(&bytes, &pinned.hash, &pinned.path)?;
                    Ok(Some(bytes))
                }
                Err(_) => Ok(None),
            }
        } else {
            Ok(None)
        }
    }

    async fn snapshot(&mut self, object_store: &Operator) -> Result<String> {
        super::snapshot(object_store, Some(self.pin_path.as_str()), &self.source_path).await
    }
}

// --- Store verification ---
//
// The checks below are specific to how the `Spawn` pinner stores data: a
// content-addressed store of files at `<hash prefix>/<hash rest>` paths, with
// each migration's lock.toml recording a root hash into that same store. A
// different pinner (e.g. one backed by git history) wouldn't have a local
// content-addressed store to walk at all, so "verify" for it would mean
// something else entirely — this isn't a `Pinner`-trait-level concept yet.

/// A pinned file whose contents no longer hash to the value encoded in its path.
#[derive(Debug)]
pub struct CorruptedFile {
    pub path: String,
    pub expected_hash: String,
    pub actual_hash: String,
}

/// A migration whose lock.toml points at a root hash that isn't in the store.
#[derive(Debug)]
pub struct MissingRoot {
    pub migration: String,
    pub hash: String,
}

/// Result of verifying the Spawn pinned store.
#[derive(Debug, Default)]
pub struct VerifyResult {
    /// Number of pinned files whose content hash was checked.
    pub checked_count: usize,
    /// Pinned files whose contents don't match their path-encoded hash.
    pub corrupted: Vec<CorruptedFile>,
    /// Migrations whose lock.toml root hash has no corresponding file in the store.
    pub missing_roots: Vec<MissingRoot>,
}

/// Walks every file in the pinned folder and recomputes its content hash,
/// comparing it against the hash encoded in its path. Unlike `verify_hash`,
/// this collects every mismatch instead of stopping at the first one.
async fn verify_store_contents(
    fs: &Operator,
    pinned_folder: &str,
) -> Result<(usize, Vec<CorruptedFile>)> {
    let files = super::list_store_files(fs, pinned_folder).await?;

    let mut checked_count = 0;
    let mut corrupted = Vec::new();

    for file in files {
        let contents = fs
            .read(&file.path)
            .await
            .with_context(|| format!("failed to read pinned file at {}", file.path))?
            .to_bytes();
        let actual_hash = content_hash(&contents);
        let expected_hash = file.hash();

        checked_count += 1;
        if actual_hash != expected_hash {
            corrupted.push(CorruptedFile {
                path: file.path,
                expected_hash,
                actual_hash,
            });
        }
    }

    Ok((checked_count, corrupted))
}

/// Checks that every migration's lock.toml root hash has a corresponding file
/// present in the pinned store.
async fn verify_lock_roots(fs: &Operator, pather: &FolderPather) -> Result<Vec<MissingRoot>> {
    let mut missing = Vec::new();

    let statuses = list_migration_fs_status(fs, pather, None).await?;

    for (name, status) in statuses {
        if !status.has_lock_toml {
            continue;
        }

        let lock_path = pather.migration_lock_file_path(&name);
        let lock_data = pinfile::load_lock_file(fs, &lock_path)
            .await
            .with_context(|| format!("failed to load lock file for migration {}", name))?;

        let root_path = format!(
            "{}/{}",
            pather.pinned_folder(),
            super::hash_to_path(&lock_data.pin)?
        );
        if !fs.exists(&root_path).await? {
            missing.push(MissingRoot {
                migration: name,
                hash: lock_data.pin,
            });
        }
    }

    Ok(missing)
}

/// Verifies the Spawn pinned store: every file's contents must match its
/// path-encoded hash, and every migration's lock.toml root hash must exist
/// in the store.
pub async fn verify_store(fs: &Operator, pather: &FolderPather) -> Result<VerifyResult> {
    let (checked_count, corrupted) = verify_store_contents(fs, &pather.pinned_folder()).await?;
    let missing_roots = verify_lock_roots(fs, pather).await?;

    Ok(VerifyResult {
        checked_count,
        corrupted,
        missing_roots,
    })
}
