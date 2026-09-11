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
    pub async fn create_migration(&self, template: Option<String>) -> Result<String> {
        // TODO: return error if migration already exists.
        let path = self.config.pather().migration_folder(&self.name);

        let script_path = format!("{}/up.sql", &path);
        println!("creating migration at {}", &script_path);
        self.config
            .operator()
            .write(
                &script_path,
                template.unwrap_or_else(|| BASE_MIGRATION.to_string()),
            )
            .await?;

        Ok(self.name.to_string())
    }

    /// Opens the specified script file and returns a streaming generation that can
    /// render directly to a writer without materializing the entire SQL in memory.
    pub async fn generate_streaming(
        &self,
        variables: Option<crate::variables::Variables>,
        secrets_mode: crate::secrets::SecretsRenderMode,
    ) -> Result<template::StreamingGeneration> {
        let lock_file = if self.use_pinned {
            let path = self.config.pather().migration_lock_file_path(&self.name);
            Some(path)
        } else {
            None
        };
        let script_path = &self.config.pather().migration_script_file_path(&self.name);
        template::generate_streaming(
            &self.config,
            lock_file,
            &self.name,
            template::ScriptType::Migration,
            script_path,
            variables,
            secrets_mode,
        )
        .await
    }

    /// The pin hash `--no-pin` should record: freshly recomputed from the
    /// current, live component tree on every call, never read from
    /// `lock.toml`. A pinned migration's pin hash instead comes from
    /// `StreamingGeneration::pin_hash()`, captured from the same lock read
    /// used to render, so it can never drift from what was actually
    /// rendered — the way a second, independent read here once could.
    /// `snapshot`'s `store_path: None` computes the same root hash a real
    /// pin would, without writing anything to the persistent `pinned/` store.
    pub async fn recompute_pin_hash(&self) -> Result<String> {
        crate::store::pinner::snapshot(
            self.config.operator(),
            None,
            &self.config.pather().components_folder(),
        )
        .await
    }
}
