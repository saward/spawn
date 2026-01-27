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

use crate::config::Config;
use crate::engine::{MigrationDbInfo, MigrationHistoryStatus};
use crate::store::pinner::latest::Latest;
use crate::store::Store;
use anyhow::Result;
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
/// This is shared logic that can be used by multiple commands (status, apply_all, etc.)
pub async fn get_combined_migration_status(
    config: &Config,
    namespace: &str,
) -> Result<Vec<MigrationStatusRow>> {
    let engine = config.new_engine().await?;
    let op = config.operator();

    // Get all migrations from the filesystem
    let pinner = Latest::new("")?;
    let pather = config.pather();
    let store = Store::new(Box::new(pinner), op.clone(), pather)?;

    let fs_migrations = store.list_migrations().await?;

    // Extract migration names from paths
    let fs_migration_names: HashSet<String> = fs_migrations
        .iter()
        .filter_map(|path| {
            path.trim_end_matches('/')
                .rsplit('/')
                .next()
                .map(|s| s.to_string())
        })
        .collect();

    // Get all migrations from database with their latest history entry
    let db_migrations_list = engine.get_migrations_from_db(Some(namespace)).await?;

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
