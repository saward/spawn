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
pub fn complete_migrations(_current: &OsStr) -> Vec<CompletionCandidate> {
    list_migrations()
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
fn list_migrations() -> Vec<String> {
    with_runtime(|op, pather| async move {
        list_migration_fs_status(&op, &pather, None)
            .await
            .map(|statuses| statuses.into_keys().collect())
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
        let service = Fs::default().root(".");
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
    fn test_complete_migrations_returns_empty_without_config() {
        // When run from repo root without spawn.toml, should return empty
        let result = complete_migrations(&OsString::new());
        // Just verify it doesn't panic - result depends on working directory
        let _ = result;
    }
}
