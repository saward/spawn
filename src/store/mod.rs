use crate::config::FolderPather;
use anyhow::{Context, Result};
use futures::TryStreamExt;
use include_dir::{Dir, DirEntry};
use opendal::services::Memory;
use opendal::Operator;
use std::collections::BTreeMap;
use std::fmt::Debug;

use crate::store::pinner::Pinner;

pub mod pinner;

/// Filesystem-level status of a single migration.
#[derive(Debug, Clone)]
pub struct MigrationFileStatus {
    pub has_up_sql: bool,
    pub has_lock_toml: bool,
}

/// Get the filesystem status of a single migration.
/// Returns the status indicating whether up.sql and lock.toml files exist.
pub async fn get_migration_fs_status(
    op: &Operator,
    pather: &FolderPather,
    migration_name: &str,
) -> Result<MigrationFileStatus> {
    // List only this specific migration folder
    let statuses = list_migration_fs_status(op, pather, Some(migration_name)).await?;

    Ok(statuses
        .get(migration_name)
        .cloned()
        .unwrap_or(MigrationFileStatus {
            has_up_sql: false,
            has_lock_toml: false,
        }))
}

/// Scan the migrations folder and return the filesystem status of each migration,
/// keyed by migration name. This does not touch the database.
/// Performs a single recursive list for efficiency with remote storage.
///
/// If `migration_name` is provided, only lists that specific migration folder.
/// Otherwise lists all migrations.
pub async fn list_migration_fs_status(
    op: &Operator,
    pather: &FolderPather,
    migration_name: Option<&str>,
) -> Result<BTreeMap<String, MigrationFileStatus>> {
    let migrations_folder = pather.migrations_folder();
    // Normalize the prefix to match opendal's path normalization:
    // opendal strips leading "./" and "/" from entry.path() results,
    // so we must do the same for strip_prefix to work correctly.
    let normalized_folder = migrations_folder
        .trim_start_matches("./")
        .trim_start_matches('/');
    let migrations_prefix = if let Some(name) = migration_name {
        // List only the specific migration folder
        format!("{}/{}/", normalized_folder, name)
    } else {
        // List all migrations
        format!("{}/", normalized_folder)
    };

    // Single recursive list - efficient for remote storage like S3
    let mut lister = op
        .lister_with(&migrations_prefix)
        .recursive(true)
        .await
        .context("listing migrations")?;

    let mut result: BTreeMap<String, MigrationFileStatus> = BTreeMap::new();

    while let Some(entry) = lister.try_next().await? {
        let path = entry.path().to_string();
        let relative_path = path.strip_prefix(&migrations_prefix).unwrap_or(&path);

        // When listing a specific migration, relative_path is just "up.sql" or "lock.toml".
        // When listing all migrations, relative_path is "migration-name/up.sql" etc.
        // Resolve the migration name and filename from the relative path.
        let (name, filename) = match relative_path.split_once('/') {
            Some((name, filename)) => (name, filename),
            None if migration_name.is_some() => (migration_name.unwrap(), relative_path),
            None => continue,
        };

        let status = result
            .entry(name.to_string())
            .or_insert(MigrationFileStatus {
                has_up_sql: false,
                has_lock_toml: false,
            });

        if filename == "up.sql" {
            status.has_up_sql = true;
        } else if filename == "lock.toml" {
            status.has_lock_toml = true;
        }
    }

    Ok(result)
}

pub struct Store {
    pinner: Box<dyn Pinner>,
    fs: Operator,
    pather: FolderPather,
}

impl Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("fs", &self.fs).finish()
    }
}

impl Store {
    pub fn new(pinner: Box<dyn Pinner>, fs: Operator, pather: FolderPather) -> Result<Store> {
        Ok(Store { pinner, fs, pather })
    }

    pub async fn load_component(&self, name: &str) -> Result<Option<String>> {
        let res = self.pinner.load(name, &self.fs).await?;

        Ok(res)
    }

    pub async fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let full_path = format!("{}/{}", self.pather.components_folder(), path);
        let result = self.fs.read(&full_path).await?;
        Ok(result.to_bytes().to_vec())
    }

    pub async fn load_migration(&self, name: &str) -> Result<String> {
        let result = self.fs.read(&name).await?;
        let bytes = result.to_bytes();
        let contents = String::from_utf8(bytes.to_vec())?;

        Ok(contents)
    }

    pub async fn list_migrations(&self) -> Result<Vec<String>> {
        let mut migrations: Vec<String> = Vec::new();
        let mut fs_lister = self
            .fs
            .lister(format!("{}/", &self.pather.migrations_folder()).as_ref())
            .await?;
        while let Some(entry) = fs_lister.try_next().await? {
            let path = entry.path().to_string();
            if path.ends_with("/") {
                migrations.push(path)
            }
        }

        // Sort migrations by name to ensure oldest to newest order
        migrations.sort();

        Ok(migrations)
    }
}

pub enum DesiredOperator {
    Memory,
    FileSystem,
}

// Handy function for getting a disk based folder of data and return an in
// memory operator that has the same contents.  Particularly useful for tests.
pub async fn disk_to_operator(
    source_folder: &str,
    dest_prefix: Option<&str>,
    desired_operator: DesiredOperator,
) -> Result<Operator> {
    let dest_op = match desired_operator {
        DesiredOperator::FileSystem => {
            let dest_service = opendal::services::Fs::default().root("./testout");
            Operator::new(dest_service)?.finish()
        }
        DesiredOperator::Memory => {
            let dest_service = Memory::default();
            Operator::new(dest_service)?.finish()
        }
    };

    // Create a LocalFileSystem to read from static/example
    let fs_service = opendal::services::Fs::default().root(source_folder);
    let source_store = Operator::new(fs_service)
        .context("disk_to_mem_operator failed to create operator")?
        .finish();

    // Populate the in-memory store with contents from static/example
    let store_loc = dest_prefix.unwrap_or_default();
    crate::store::populate_store_from_store(&source_store, &dest_op, "", store_loc)
        .await
        .context("call to populate memory fs from object store")?;

    Ok(dest_op)
}

pub async fn populate_store_from_store(
    source_store: &Operator,
    target_store: &Operator,
    source_prefix: &str,
    dest_prefix: &str,
) -> Result<()> {
    let mut lister = source_store
        .lister_with(source_prefix)
        .recursive(true)
        .await
        .context("lister call")?;
    let mut list_result: Vec<opendal::Entry> = Vec::new();

    while let Some(entry) = lister.try_next().await? {
        if entry.path().ends_with("/") {
            continue;
        }
        list_result.push(entry);
    }

    for entry in list_result {
        // Print out the file we're writing:
        let dest_object_path = format!("{}{}", dest_prefix, entry.path());
        let source_object_path = entry.path();

        // Get the object data
        let bytes = source_store
            .read(&source_object_path)
            .await
            .context(format!("read path {}", &source_object_path))?;

        // Store in target with the same path
        target_store
            .write(&dest_object_path, bytes)
            .await
            .context("write")?;
    }

    Ok(())
}

/// Creates a memory-based OpenDAL operator from an include_dir bundle.
///
/// This function takes a bundled directory (created with include_dir!) and
/// creates an in-memory OpenDAL operator containing all files from that bundle.
/// This is useful for embedding migrations, templates, or other static files
/// directly into the binary while still being able to use OpenDAL's interface.
///
/// # Arguments
///
/// * `included_dir` - A reference to a Dir created with include_dir! macro
/// * `dest_prefix` - Optional prefix to add to all file paths in the operator
///
/// # Returns
///
/// A memory-based OpenDAL operator containing all files from the bundled directory
pub async fn operator_from_includedir(
    dir: &Dir<'_>,
    dest_prefix: Option<&str>,
) -> Result<Operator> {
    // Create a memory operator
    let dest_service = Memory::default();
    let operator = Operator::new(dest_service)?.finish();

    let prefix = dest_prefix.unwrap_or_default();

    // First collect all file information
    let mut files_to_write = Vec::new();
    collect_files_from_dir(dir, "", &mut files_to_write);

    // Then write all files to the operator
    for (dest_path, contents) in &files_to_write {
        let final_path = format!("{}{}", prefix, dest_path);
        operator
            .write(&final_path, contents.clone())
            .await
            .context(format!("Failed to write file {}", final_path))?;
    }

    Ok(operator)
}

// Helper function to recursively collect file information
fn collect_files_from_dir(dir: &Dir<'_>, current_path: &str, files: &mut Vec<(String, Vec<u8>)>) {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(subdir) => {
                let new_path = if current_path.is_empty() {
                    subdir.path().to_string_lossy().to_string()
                } else {
                    format!(
                        "{}/{}",
                        current_path,
                        subdir.path().file_name().unwrap().to_string_lossy()
                    )
                };
                collect_files_from_dir(subdir, &new_path, files);
            }
            DirEntry::File(file) => {
                let file_path = if current_path.is_empty() {
                    file.path().to_string_lossy().to_string()
                } else {
                    format!(
                        "{}/{}",
                        current_path,
                        file.path().file_name().unwrap().to_string_lossy()
                    )
                };
                files.push((file_path, file.contents().to_vec()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::pinner::latest::Latest;
    use include_dir::{include_dir, Dir};

    // Create a test directory structure for testing
    static TEST_DIR: Dir<'_> = include_dir!("./static");

    #[tokio::test]
    async fn test_operator_from_includedir_with_prefix() {
        let result = operator_from_includedir(&TEST_DIR, Some("test-prefix/")).await;
        assert!(
            result.is_ok(),
            "Should create operator with prefix successfully"
        );
    }

    #[tokio::test]
    async fn test_list_migrations_returns_two_migrations() {
        // Load the two_migrations test folder into a memory operator
        let op = disk_to_operator(
            "./static/tests/two_migrations",
            None,
            DesiredOperator::Memory,
        )
        .await
        .expect("Failed to create operator from disk");

        // Create a pinner and FolderPather
        let pinner = Latest::new("").expect("Failed to create Latest pinner");
        let pather = FolderPather {
            spawn_folder: "".to_string(),
        };

        // Create the Store
        let store = Store::new(Box::new(pinner), op, pather).expect("Failed to create Store");

        // Call list_migrations and verify the result
        let migrations: Vec<String> = store
            .list_migrations()
            .await
            .expect("Failed to list migrations");

        // Should have exactly 2 migrations
        assert_eq!(
            migrations.len(),
            2,
            "Expected 2 migrations, got {:?}",
            migrations
        );

        // Verify the migration names are present (they end with /)
        let migration_names: Vec<&str> = migrations.iter().map(|s| s.as_str()).collect();
        assert!(
            migration_names
                .iter()
                .any(|m| m.contains("20240907212659-initial")),
            "Expected to find 20240907212659-initial migration, got {:?}",
            migration_names
        );
        assert!(
            migration_names
                .iter()
                .any(|m| m.contains("20240908123456-second")),
            "Expected to find 20240908123456-second migration, got {:?}",
            migration_names
        );
    }

    #[tokio::test]
    async fn test_get_migration_fs_status_with_lock() {
        // Create an in-memory operator with both up.sql and lock.toml
        let mem_service = Memory::default();
        let op = Operator::new(mem_service).unwrap().finish();

        // Write both up.sql and lock.toml
        op.write("/migrations/20240101000000-test/up.sql", "SELECT 1;")
            .await
            .expect("Failed to write up.sql");
        op.write(
            "/migrations/20240101000000-test/lock.toml",
            "pin = \"abc123\"",
        )
        .await
        .expect("Failed to write lock.toml");

        let pather = FolderPather {
            spawn_folder: "".to_string(),
        };

        // This migration should have both up.sql and lock.toml
        let status = get_migration_fs_status(&op, &pather, "20240101000000-test")
            .await
            .expect("Failed to get migration status");

        assert!(status.has_up_sql, "Migration should have up.sql");
        assert!(status.has_lock_toml, "Migration should have lock.toml");
    }

    #[tokio::test]
    async fn test_get_migration_fs_status_without_lock() {
        // Create an in-memory operator with just an up.sql file
        let mem_service = Memory::default();
        let op = Operator::new(mem_service).unwrap().finish();

        // Write only up.sql, no lock.toml
        op.write("/migrations/20240101000000-test/up.sql", "SELECT 1;")
            .await
            .expect("Failed to write up.sql");

        let pather = FolderPather {
            spawn_folder: "".to_string(),
        };

        let status = get_migration_fs_status(&op, &pather, "20240101000000-test")
            .await
            .expect("Failed to get migration status");

        assert!(status.has_up_sql, "Migration should have up.sql");
        assert!(!status.has_lock_toml, "Migration should not have lock.toml");
    }

    #[tokio::test]
    async fn test_list_migration_fs_status() {
        // Test listing works for different spawn_folder formats, including
        // "./" prefix which opendal normalizes away from entry.path().
        for spawn_folder in [
            "",
            "./database/spawn",
            "/database/spawn",
            "./spawn",
            "/spawn",
        ] {
            let mem_service = Memory::default();
            let op = Operator::new(mem_service).unwrap().finish();

            let prefix = spawn_folder
                .trim_start_matches("./")
                .trim_start_matches('/');
            let migrations = if prefix.is_empty() {
                "migrations".to_string()
            } else {
                format!("{}/migrations", prefix)
            };

            op.write(
                &format!("{}/20240101-first/up.sql", migrations),
                "SELECT 1;",
            )
            .await
            .unwrap();
            op.write(
                &format!("{}/20240101-first/lock.toml", migrations),
                "pin = \"abc\"",
            )
            .await
            .unwrap();
            op.write(
                &format!("{}/20240102-second/up.sql", migrations),
                "SELECT 2;",
            )
            .await
            .unwrap();

            let pather = FolderPather {
                spawn_folder: spawn_folder.to_string(),
            };

            for filter in [None, Some("20240101-first")] {
                let statuses = list_migration_fs_status(&op, &pather, filter)
                    .await
                    .expect("Failed to list migration statuses");

                let first = statuses
                    .get("20240101-first")
                    .expect("Should have first migration");
                assert!(first.has_up_sql);
                assert!(first.has_lock_toml);

                if filter.is_none() {
                    assert_eq!(statuses.len(), 2);
                    let second = statuses
                        .get("20240102-second")
                        .expect("Should have second migration");
                    assert!(second.has_up_sql);
                    assert!(!second.has_lock_toml);
                } else {
                    assert_eq!(statuses.len(), 1);
                }
            }
        }
    }
}
