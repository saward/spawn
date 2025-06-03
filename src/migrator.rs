use crate::config;
use crate::store::{self, Store};
use crate::template;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context, Result};
use minijinja::context;
use serde::Serialize;

static BASE_MIGRATION: &str = "BEGIN;

COMMIT;
";

/// Final SQL output generator
#[derive(Debug)]
pub struct Migrator {
    config: config::Config,
    /// Path for the script itself, set to the location under the migrations
    /// folder.
    script_path: OsString,
    /// Whether to use pinned components
    use_pinned: bool,
}

impl Migrator {
    pub fn new(config: &config::Config, script_path: OsString, use_pinned: bool) -> Self {
        Migrator {
            config: config.clone(),
            script_path,
            use_pinned,
        }
    }

    /// Creates the migration folder with blank setup.
    pub fn create_migration(&self) -> Result<()> {
        // Todo: return error if migration already exists.
        let path = self.config.migration_folder(&self.script_path);
        if path.exists() {
            return Err(anyhow::anyhow!(
                "folder for migration {:?} already exists, aborting.",
                path,
            ));
        }
        fs::create_dir_all(&path)?;

        // Create our blank script file:
        fs::write(&path.join("script.sql"), BASE_MIGRATION)?;

        Ok(())
    }

    pub fn script_file_path(&self) -> Result<PathBuf> {
        let path = self.config.migration_script_file_path(&self.script_path);
        Ok(path
            .canonicalize()
            .context(format!("Invalid script path for '{:?}'", path))?)
    }

    /// Opens the specified script file and generates a migration script, compiled
    /// using minijinja.
    pub fn generate(
        &self,
        variables: Option<crate::variables::Variables>,
    ) -> Result<template::Generation> {
        let lock_file = if self.use_pinned {
            Some(self.config.migration_lock_file_path(&self.script_path))
        } else {
            None
        };

        // Add our migration script to environment:
        let full_script_path = self.script_file_path()?;
        let contents = std::fs::read_to_string(&full_script_path).context(format!(
            "Failed to read migration script '{}'",
            full_script_path.display()
        ))?;

        template::generate(&self.config, lock_file, &contents, variables)
    }
}

#[cfg(test)]
mod tests {
    use crate::migrator::Migrator;
    use std::{ffi::OsString, path::PathBuf};

    // fn test_config() -> Migrator {
    //     Migrator::new(
    //         PathBuf::from("./base_folder"),
    //         OsString::from("subfolder/migration_script"),
    //         false,
    //     )
    // }
    //
    // #[test]
    // fn script_file_path() {
    //     let config = test_config();
    //     assert_eq!(
    //         PathBuf::from("./base_folder/migrations/subfolder/migration_script"),
    //         config.script_file_path(),
    //     );
    // }
    //
    // #[test]
    // fn lock_file_path() {
    //     let config = test_config();
    //     assert_eq!(
    //         PathBuf::from("./base_folder/migrations/subfolder/migration_script.lock"),
    //         config.lock_file_path(),
    //     );
    // }
}
