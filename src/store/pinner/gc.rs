//! Garbage collection for the pinned content-addressable store.
//!
//! Finds and removes pinned files that are no longer referenced by any migration's lock.toml.

use anyhow::{Context, Result};
use opendal::Operator;
use std::collections::HashSet;

use crate::config::FolderPather;
use crate::pinfile;
use crate::store::list_migration_fs_status;

use super::{hash_to_path, read_hash_file, EntryKind, Tree};

/// Result of a garbage collection operation.
#[derive(Debug)]
pub struct GcResult {
    /// Hashes that were deleted (or would be deleted in dry-run mode).
    pub orphaned: Vec<String>,
    /// Total number of hashes that are still referenced.
    pub referenced_count: usize,
    /// Whether this was a dry run.
    pub dry_run: bool,
}

/// Recursively walk a tree and collect all referenced hashes (both tree and blob).
async fn walk_tree_hashes(
    fs: &Operator,
    store_path: &str,
    hash: &str,
    referenced: &mut HashSet<String>,
) -> Result<()> {
    // Add this hash itself
    referenced.insert(hash.to_string());

    // Read and parse tree
    let contents = read_hash_file(fs, store_path, hash)
        .await
        .with_context(|| format!("failed to read tree hash {}", hash))?;
    let tree: Tree = toml::from_str(&contents)
        .with_context(|| format!("failed to parse tree TOML for hash {}", hash))?;

    for entry in tree.entries {
        match entry.kind {
            EntryKind::Blob => {
                referenced.insert(entry.hash);
            }
            EntryKind::Tree => {
                Box::pin(walk_tree_hashes(fs, store_path, &entry.hash, referenced)).await?;
            }
        }
    }

    Ok(())
}

/// Collect all hashes referenced by all migrations' lock.toml files.
async fn collect_referenced_hashes(
    fs: &Operator,
    pather: &FolderPather,
) -> Result<HashSet<String>> {
    let mut referenced = HashSet::new();

    // List all migrations
    let statuses = list_migration_fs_status(fs, pather, None).await?;

    for (name, status) in statuses {
        if !status.has_lock_toml {
            continue;
        }

        // Load lock.toml
        let lock_path = pather.migration_lock_file_path(&name);
        let lock_data = pinfile::load_lock_file(fs, &lock_path)
            .await
            .with_context(|| format!("failed to load lock file for migration {}", name))?;

        // Walk tree and collect all hashes
        walk_tree_hashes(fs, &pather.pinned_folder(), &lock_data.pin, &mut referenced)
            .await
            .with_context(|| format!("failed to walk tree for migration {}", name))?;
    }

    Ok(referenced)
}

/// List all files in the pinned folder and return their hashes.
async fn list_pinned_hashes(fs: &Operator, pinned_folder: &str) -> Result<HashSet<String>> {
    Ok(super::list_store_files(fs, pinned_folder)
        .await?
        .into_iter()
        .map(|file| file.hash())
        .collect())
}

/// Run garbage collection on the pinned folder.
///
/// If `dry_run` is true, no files will be deleted - only reports what would be deleted.
pub async fn gc_pinned(fs: &Operator, pather: &FolderPather, dry_run: bool) -> Result<GcResult> {
    let referenced = collect_referenced_hashes(fs, pather).await?;
    let existing = list_pinned_hashes(fs, &pather.pinned_folder()).await?;

    let orphaned: Vec<String> = existing.difference(&referenced).cloned().collect();

    if !dry_run {
        for hash in &orphaned {
            let path = format!("{}/{}", pather.pinned_folder(), hash_to_path(hash)?);
            fs.delete(&path).await.with_context(|| {
                format!("failed to delete orphaned pinned file with hash {}", hash)
            })?;
        }
    }

    Ok(GcResult {
        orphaned,
        referenced_count: referenced.len(),
        dry_run,
    })
}
