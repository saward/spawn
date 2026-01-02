use crate::config;
use crate::template;

use anyhow::Result;

static BASE_MIGRATION: &str = "BEGIN;

COMMIT;
";

/// Final SQL output generator
#[derive(Debug)]
pub struct Migrator {
    config: config::Config,
    /// Name of the migration, as an object store path.
    name: String,
    /// Whether to use pinned components
    use_pinned: bool,
}

impl Migrator {
    pub fn new(config: &config::Config, name: &str, use_pinned: bool) -> Self {
        Migrator {
            config: config.clone(),
            name: name.to_string(),
            use_pinned,
        }
    }

    /// Creates the migration folder with blank setup.
    pub async fn create_migration(&self) -> Result<String> {
        // TODO: return error if migration already exists.
        let path = self.config.migration_folder(&self.name);

        let script_path = format!("{}/up.sql", &path);
        println!("creating migration at {}", &script_path);
        self.config
            .operator()
            .write(&script_path, BASE_MIGRATION)
            .await?;

        Ok(self.name.to_string())
    }

    /// Opens the specified script file and generates a migration script, compiled
    /// using minijinja.
    pub async fn generate(
        &self,
        variables: Option<crate::variables::Variables>,
    ) -> Result<template::Generation> {
        let lock_file = if self.use_pinned {
            let path = self.config.migration_lock_file_path(&self.name);
            Some(path)
        } else {
            None
        };
        let script_path = &self.config.migration_script_file_path(&self.name);
        println!("generate script path: {}", script_path);
        template::generate(&self.config, lock_file, script_path, variables).await
    }
}
