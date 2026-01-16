use crate::config::FolderPather;
use anyhow::{Context, Result};
use futures::TryStreamExt;
use include_dir::{Dir, DirEntry};
use opendal::services::Memory;
use opendal::Operator;
use std::fmt::Debug;

use crate::store::pinner::Pinner;

pub mod pinner;

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
    for (dest_path, contents) in files_to_write {
        let final_path = format!("{}{}", prefix, dest_path);
        operator
            .write(&final_path, contents)
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
}
