use crate::config;
use crate::template;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

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
    pub fn create_migration(&self) -> Result<String> {
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
        fs::write(&path.join("up.sql"), BASE_MIGRATION)?;

        let name = path
            .file_name()
            .ok_or(anyhow::anyhow!("couldn't find name for created migration"))?;

        Ok(name.to_string_lossy().to_string())
    }

    pub fn script_file_path(&self) -> Result<PathBuf> {
        let path = self.config.migration_script_file_path(&self.script_path);
        Ok(path
            .canonicalize()
            .context(format!("Invalid script path for '{:?}'", path))?)
    }

    /// Opens the specified script file and generates a migration script, compiled
    /// using minijinja.
    pub async fn generate(
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

        template::generate(&self.config, lock_file, &contents, variables).await
    }
}
