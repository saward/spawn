use crate::config;
use crate::template;
use std::fs;

use anyhow::{Context, Result};
use object_store::local::LocalFileSystem;
use object_store::path::Path;
use object_store::ObjectStore;

static BASE_MIGRATION: &str = "BEGIN;

COMMIT;
";

/// Final SQL output generator
#[derive(Debug)]
pub struct Migrator {
    config: config::Config,
    /// Name of the migration, as an object store path.
    name: Path,
    /// Whether to use pinned components
    use_pinned: bool,
}

impl Migrator {
    pub fn new(config: &config::Config, name: Path, use_pinned: bool) -> Self {
        Migrator {
            config: config.clone(),
            name,
            use_pinned,
        }
    }

    /// Creates the migration folder with blank setup.
    pub fn create_migration(&self) -> Result<String> {
        // TODO: return error if migration already exists.
        let path = self.config.migration_folder(&self.name);
        // TODO: this should use object store
        let path_str = path.as_ref();
        if std::path::Path::new(path_str).exists() {
            return Err(anyhow::anyhow!(
                "folder for migration {:?} already exists, aborting.",
                path_str,
            ));
        }
        fs::create_dir_all(path_str)?;

        // Create our blank script file:
        // TODO: use proper join/child here
        let script_path = format!("{}/up.sql", path_str);
        fs::write(&script_path, BASE_MIGRATION)?;

        // TODO: change this to use filename from object store path object
        let name = path_str
            .split('/')
            .last()
            .ok_or(anyhow::anyhow!("couldn't find name for created migration"))?;

        Ok(name.to_string())
    }

    /// Opens the specified script file and generates a migration script, compiled
    /// using minijinja.
    pub async fn generate(
        &self,
        variables: Option<crate::variables::Variables>,
    ) -> Result<template::Generation> {
        let lock_file = if self.use_pinned {
            let path = self.config.migration_lock_file_path(&self.name);
            Some(path.as_ref().to_string())
        } else {
            None
        };

        // Create and set up the component loader
        let fs: Box<dyn ObjectStore> = Box::new(LocalFileSystem::new_with_prefix(
            self.config.spawn_folder_path().as_ref(),
        )?);

        template::generate(&self.config, lock_file, &self.name, variables, fs).await
    }
}
