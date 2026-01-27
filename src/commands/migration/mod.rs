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
pub use pin::PinMigration;
pub use status::MigrationStatus;

pub const DEFAULT_NAMESPACE: &str = "default";

use crate::config::Config;
use crate::engine::{MigrationDbInfo, MigrationHistoryStatus};
use crate::store::pinner::latest::Latest;
use crate::store::Store;
use anyhow::Result;
use dialoguer::Confirm;
use std::collections::{HashMap, HashSet};

/// Combined status of a migration from both filesystem and database
#[derive(Debug, Clone)]
pub struct MigrationStatusRow {
    pub migration_name: String,
    pub exists_in_filesystem: bool,
    pub exists_in_db: bool,
    pub last_status: Option<MigrationHistoryStatus>,
    pub last_activity: Option<String>,
    pub checksum: Option<String>,
}

/// Get the combined status of all migrations from both filesystem and database.
/// If namespace is None, returns migrations from all namespaces.
/// This is shared logic that can be used by multiple commands (status, apply_all, etc.)
pub async fn get_combined_migration_status(
    config: &Config,
    namespace: Option<&str>,
) -> Result<Vec<MigrationStatusRow>> {
    let engine = config.new_engine().await?;
    let op = config.operator();

    // Get all migrations from the filesystem
    let pinner = Latest::new("")?;
    let pather = config.pather();
    let store = Store::new(Box::new(pinner), op.clone(), pather)?;

    let fs_migrations = store.list_migrations().await?;

    // Extract migration names from paths, filtering out the parent directory
    let fs_migration_names: HashSet<String> = fs_migrations
        .iter()
        .filter_map(|path| {
            let name = path.trim_end_matches('/').rsplit('/').next()?;
            // Filter out entries that don't look like migration names
            // Migration names should start with a timestamp (digits) or be non-empty
            // Also filter out the parent folder, which seems to be prefixed with
            // "./".
            if name.is_empty() || path.starts_with("./") {
                None
            } else {
                Some(name.to_string())
            }
        })
        .collect();

    // Get all migrations from database with their latest history entry
    let db_migrations_list = engine.get_migrations_from_db(namespace).await?;

    // Convert to a map for easier lookup
    let db_migrations: HashMap<String, MigrationDbInfo> = db_migrations_list
        .into_iter()
        .map(|info| (info.migration_name.clone(), info))
        .collect();

    // Combine both sources
    let all_migration_names: HashSet<String> = fs_migration_names
        .iter()
        .chain(db_migrations.keys())
        .cloned()
        .collect();

    let mut results: Vec<MigrationStatusRow> = all_migration_names
        .into_iter()
        .map(|name| {
            let exists_in_fs = fs_migration_names.contains(&name);
            let db_info = db_migrations.get(&name);

            MigrationStatusRow {
                migration_name: name,
                exists_in_filesystem: exists_in_fs,
                exists_in_db: db_info.is_some(),
                last_status: db_info.and_then(|info| info.last_status),
                last_activity: db_info.and_then(|info| info.last_activity.clone()),
                checksum: db_info.and_then(|info| info.checksum.clone()),
            }
        })
        .collect();

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

    let db_config = config.db_config()?;
    let target = config.database.as_deref().unwrap_or("unknown");
    let env = &db_config.environment;

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
