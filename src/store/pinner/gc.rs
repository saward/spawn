//! Garbage collection for the pinned content-addressable store.
//!
//! Finds and removes pinned files no longer referenced by any migration or script.

use anyhow::{Context, Result};
use opendal::Operator;
use std::collections::HashSet;

use crate::config::FolderPather;
use crate::source_config::{load_source_config, SourceConfig};
use crate::store::{list_migration_fs_status, list_script_fs_status};

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

/// Collect every hash referenced by a migration's lock.toml or a script's config.toml.
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
        let source_config: SourceConfig = load_source_config(fs, &lock_path)
            .await
            .with_context(|| format!("failed to load lock file for migration {}", name))?;

        // Walk tree and collect all hashes
        walk_tree_hashes(
            fs,
            &pather.pinned_folder(),
            &source_config.pin,
            &mut referenced,
        )
        .await
        .with_context(|| format!("failed to walk tree for migration {}", name))?;
    }

    for (script, status) in list_script_fs_status(fs, pather).await? {
        // Only a pinned script has a config.toml; an unpinned one keeps no root alive.
        if !status.has_config_toml {
            continue;
        }

        let config_path = pather.script_config_path(&script);
        let source_config: SourceConfig = load_source_config(fs, &config_path)
            .await
            .with_context(|| format!("failed to load config for script {}", script))?;

        walk_tree_hashes(
            fs,
            &pather.pinned_folder(),
            &source_config.pin,
            &mut referenced,
        )
        .await
        .with_context(|| format!("failed to walk tree for script {}", script))?;
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
