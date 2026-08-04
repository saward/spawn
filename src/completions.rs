//! Shell completion support for spawn CLI.
//!
//! This module provides dynamic completers for migration and test names.
//! Completions work with any shell that supports clap_complete's CompleteEnv.

use crate::config::{ConfigLoaderSaver, FolderPather, DEFAULT_CONFIG_FILE};
use crate::store::{list_migration_fs_status, list_test_fs_status};
use clap_complete::CompletionCandidate;
use opendal::services::Fs;
use opendal::Operator;
use std::ffi::OsStr;

/// Complete migration names from the migrations directory.
///
/// Returns all migrations (directories containing up.sql) as candidates.
/// Filtering is left to the shell or user's fuzzy finder (e.g., fzf-tab).
pub fn complete_migrations_up(_current: &OsStr) -> Vec<CompletionCandidate> {
    list_migrations_up()
        .into_iter()
        .map(CompletionCandidate::new)
        .collect()
}

/// Complete migration revert script names from the migrations directory.
pub fn complete_migrations_down(_current: &OsStr) -> Vec<CompletionCandidate> {
    list_migrations_down()
        .into_iter()
        .map(CompletionCandidate::new)
        .collect()
}

/// Complete test names from the tests directory.
///
/// Returns all tests as candidates. Filtering is left to the shell.
pub fn complete_tests(_current: &OsStr) -> Vec<CompletionCandidate> {
    list_tests()
        .into_iter()
        .map(CompletionCandidate::new)
        .collect()
}

/// List migrations using the existing store function.
///
/// Only returns directories that contain up.sql (valid migrations).
fn list_migrations_up() -> Vec<String> {
    with_runtime(|op, pather| async move {
        list_migration_fs_status(&op, &pather, None)
            .await
            .map(|mut statuses| {
                statuses.retain(|_, mfs| mfs.has_up_sql);
                statuses.into_keys().collect()
            })
            .unwrap_or_default()
    })
}

fn list_migrations_down() -> Vec<String> {
    with_runtime(|op, pather| async move {
        list_migration_fs_status(&op, &pather, None)
            .await
            .map(|mut statuses| {
                statuses.retain(|_, mfs| mfs.has_down_sql);
                statuses.into_keys().collect()
            })
            .unwrap_or_default()
    })
}

/// List tests using the existing store function.
///
/// Only returns directories that contain test.sql (valid tests).
fn list_tests() -> Vec<String> {
    with_runtime(|op, pather| async move {
        list_test_fs_status(&op, &pather, None)
            .await
            .map(|statuses| statuses.into_keys().collect())
            .unwrap_or_default()
    })
}

/// Helper to run async code with a tokio runtime, operator, and pather.
///
/// Spins up a minimal tokio runtime and loads config to get spawn_folder.
fn with_runtime<F, Fut>(f: F) -> Vec<String>
where
    F: FnOnce(Operator, FolderPather) -> Fut,
    Fut: std::future::Future<Output = Vec<String>>,
{
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return Vec::new(),
    };

    rt.block_on(async {
        // In tests, use static/example; in production, use current directory
        #[cfg(test)]
        let root = "static/example";
        #[cfg(not(test))]
        let root = ".";

        let service = Fs::default().root(root);
        let op = match Operator::new(service) {
            Ok(op) => op.finish(),
            Err(_) => return Vec::new(),
        };

        let spawn_folder = ConfigLoaderSaver::load(DEFAULT_CONFIG_FILE, &op, None)
            .await
            .map(|c| c.spawn_folder)
            .unwrap_or_else(|_| "spawn".to_string());

        let pather = FolderPather { spawn_folder };
        f(op, pather).await
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn test_complete_migrations() {
        let result = complete_migrations_up(&OsString::new());

        // static/example has one migration: 20240907212659-initial
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].get_value().to_str().unwrap(),
            "20240907212659-initial"
        );
    }

    #[test]
    fn test_complete_tests() {
        let result = complete_tests(&OsString::new());

        // static/example has two tests
        assert_eq!(result.len(), 2);
        let names: Vec<_> = result
            .iter()
            .map(|c| c.get_value().to_str().unwrap())
            .collect();
        assert!(names.contains(&"20250607115200-example-test"));
        assert!(names.contains(&"20250607115201-example-test"));
    }
}
