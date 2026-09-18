mod adopt;
mod apply;
mod build;
mod new;
mod pin;
mod status;

pub use adopt::AdoptMigration;
pub use apply::ApplyMigration;
pub use build::BuildMigration;
pub use new::NewMigration;
pub use pin::{PinError, PinMigration};
pub use status::MigrationStatus;

pub const DEFAULT_NAMESPACE: &str = "default";

use crate::config::Config;
use crate::engine::{MigrationDbInfo, MigrationHistoryStatus};
use crate::hash::content_hash;
use crate::pinfile::load_lock_file;
use crate::store::list_migration_fs_status;
use anyhow::Result;
use dialoguer::Confirm;
use std::collections::{HashMap, HashSet};

/// Result of comparing the current lock.toml pin against the pin_hash
/// recorded in `_spawn.migration_history` at the last apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinMatchStatus {
    /// The current lock.toml pin matches what was recorded at apply time.
    Matches,
    /// The current lock.toml pin differs from what was recorded at apply time.
    Differs,
    /// A pin was recorded at apply time, but the current lock.toml is
    /// missing or unreadable, so there's nothing left to compare it to.
    Missing,
    /// Not enough information exists to compare: the migration isn't
    /// pinned, or it has never been applied while pinned.
    NotApplicable,
}

/// Combined status of a migration from both filesystem and database
#[derive(Debug, Clone)]
pub struct MigrationStatusRow {
    pub migration_name: String,
    pub exists_in_filesystem: bool,
    pub is_pinned: bool,
    pub exists_in_db: bool,
    pub last_status: Option<MigrationHistoryStatus>,
    pub last_activity: Option<String>,
    pub checksum: Option<String>,
    pub pin_hash: Option<String>,
    pub pin_matches: PinMatchStatus,
    /// Whether the current up.sql's content hash matches the checksum
    /// recorded at the last apply. `None` when there isn't enough data to
    /// compare (e.g. the file is missing, or never applied).
    pub checksum_matches: Option<bool>,
}

/// Get the combined status of all migrations from both filesystem and database.
/// If namespace is None, returns migrations from all namespaces.
/// This is shared logic that can be used by multiple commands (status, apply_all, etc.)
pub async fn get_combined_migration_status(
    config: &Config,
    namespace: Option<&str>,
) -> Result<Vec<MigrationStatusRow>> {
    let engine = config.new_engine().await?;

    // Get filesystem status
    let fs_status = list_migration_fs_status(config.operator(), &config.pather(), None).await?;

    // Get all migrations from database with their latest history entry
    let db_migrations_list = engine.get_migrations_from_db(namespace).await?;

    // Convert to a map for easier lookup
    let db_migrations: HashMap<String, MigrationDbInfo> = db_migrations_list
        .into_iter()
        .map(|info| (info.migration_name.clone(), info))
        .collect();

    // Combine both sources
    let all_migration_names: HashSet<String> = fs_status
        .keys()
        .chain(db_migrations.keys())
        .cloned()
        .collect();

    let mut results: Vec<MigrationStatusRow> = Vec::with_capacity(all_migration_names.len());
    for name in all_migration_names {
        let fs = fs_status.get(&name);
        let db_info = db_migrations.get(&name);

        let has_up_sql = fs.map_or(false, |s| s.has_up_sql);
        let has_lock_toml = fs.map_or(false, |s| s.has_lock_toml);
        let checksum = db_info.and_then(|info| info.checksum.clone());
        let pin_hash = db_info.and_then(|info| info.pin_hash.clone());

        let pin_matches = match &pin_hash {
            Some(applied_pin) if has_lock_toml => {
                let lock_path = config.pather().migration_lock_file_path(&name);
                match load_lock_file(config.operator(), &lock_path).await {
                    Ok(lock_data) if &lock_data.pin == applied_pin => PinMatchStatus::Matches,
                    Ok(_) => PinMatchStatus::Differs,
                    Err(_) => PinMatchStatus::Missing,
                }
            }
            Some(_) => PinMatchStatus::Missing,
            None => PinMatchStatus::NotApplicable,
        };

        let checksum_matches = match (&checksum, has_up_sql) {
            (Some(recorded_checksum), true) => {
                let script_path = config.pather().migration_script_file_path(&name);
                match config.operator().read(&script_path).await {
                    Ok(buf) => Some(&content_hash(&buf.to_bytes()) == recorded_checksum),
                    Err(_) => None,
                }
            }
            _ => None,
        };

        results.push(MigrationStatusRow {
            migration_name: name.clone(),
            exists_in_filesystem: has_up_sql,
            is_pinned: has_lock_toml,
            exists_in_db: db_info.is_some(),
            last_status: db_info.and_then(|info| info.last_status),
            last_activity: db_info.and_then(|info| info.last_activity.clone()),
            checksum,
            pin_hash,
            pin_matches,
            checksum_matches,
        });
    }

    // Sort by migration name for consistent output
    results.sort_by(|a, b| a.migration_name.cmp(&b.migration_name));

    Ok(results)
}

/// Get pending migrations (no status, exists on filesystem) and prompt the user
/// to confirm. Returns `Ok(Some(migrations))` if confirmed, `Ok(None)` if
/// aborted or empty.
pub async fn get_pending_and_confirm(
    config: &Config,
    action: &str,
    yes: bool,
) -> Result<Option<Vec<String>>> {
    let status_rows = get_combined_migration_status(config, Some(DEFAULT_NAMESPACE)).await?;

    let pending: Vec<String> = status_rows
        .into_iter()
        .filter(|row| row.last_status.is_none() && row.exists_in_filesystem)
        .map(|row| row.migration_name)
        .collect();

    if pending.is_empty() {
        println!("No pending migrations to {}.", action);
        return Ok(None);
    }

    let target_config = config.target_config()?;
    let target = config.target.as_deref().unwrap_or("unknown");
    let env = &target_config.environment;

    println!();
    println!("TARGET: {}", target);
    if env.starts_with("prod") {
        println!("ENVIRONMENT: {} \u{26a0}\u{fe0f}", env);
    } else {
        println!("ENVIRONMENT: {}", env);
    }
    println!();
    println!(
        "The following {} migration{} will be {}:",
        pending.len(),
        if pending.len() == 1 { "" } else { "s" },
        if action == "apply" {
            "applied"
        } else {
            "adopted"
        },
    );
    for (i, name) in pending.iter().enumerate() {
        println!("  {}. {}", i + 1, name);
    }
    println!();

    if !yes {
        let prompt = format!("Do you want to {} these migrations?", action);
        let confirmed = Confirm::new()
            .with_prompt(prompt)
            .default(false)
            .interact()?;

        if !confirmed {
            println!("Aborted.");
            return Ok(None);
        }
    }

    println!();
    Ok(Some(pending))
}
