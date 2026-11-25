use crate::config;
use crate::source_config::SourceConfig;
use crate::template;
use crate::variables::VariableDefinition;

use anyhow::{anyhow, Context, Result};
use std::collections::BTreeMap;

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
    /// The migration's `lock.toml`, read once by [`Migrator::resolve`]. Empty
    /// for a migrator built with [`Migrator::new`], which does no I/O.
    migration_config: SourceConfig,
}

impl Migrator {
    pub fn new(config: &config::Config, name: &str, use_pinned: bool) -> Self {
        Migrator {
            config: config.clone(),
            name: name.to_string(),
            use_pinned,
            migration_config: SourceConfig::default(),
        }
    }

    /// Builds a migrator with its `lock.toml` already read, so a migration
    /// asked to use its pin is never silently rendered against other
    /// components.
    ///
    /// Mirrors [`crate::script::Script::resolve`], minus the `Auto` mode:
    /// `build` opts in with `--pinned` and `apply` opts out with `--no-pin`,
    /// so the caller always knows which it wants.
    pub async fn resolve(config: &config::Config, name: &str, use_pinned: bool) -> Result<Self> {
        let migrator = Migrator::new(config, name, use_pinned);
        // Read once here rather than per question: the same file answers both
        // whether the migration is pinned and what variables it declares.
        let migration_config = migrator.load_migration_config().await?;

        if use_pinned && migration_config.pin.is_none() {
            return Err(anyhow!(
                "migration '{}' is not pinned: run `spawn migration pin {}`",
                name,
                name
            ));
        }

        Ok(Migrator {
            migration_config,
            ..migrator
        })
    }

    /// Reads the migration's `lock.toml`, or an empty config where there is
    /// none.
    ///
    /// The file's mere existence does not mean the migration is pinned: it may
    /// be there only to declare variables. Ask [`SourceConfig::pin`] instead.
    pub async fn load_migration_config(&self) -> Result<SourceConfig> {
        let path = self.config.pather().migration_lock_file_path(&self.name);
        let exists = self
            .config
            .operator()
            .exists(&path)
            .await
            .with_context(|| format!("failed to check lock file for migration '{}'", self.name))?;
        if !exists {
            return Ok(SourceConfig::default());
        }
        self.config
            .load_source_config(&path)
            .await
            .with_context(|| format!("failed to load lock file for migration '{}'", self.name))
    }

    /// Whether this migration has been pinned, as recorded by the `lock.toml`
    /// [`Migrator::resolve`] read.
    pub fn is_pinned(&self) -> bool {
        self.migration_config.pin.is_some()
    }

    /// The variables this migration declares, in name order.
    pub fn declared_variables(&self) -> &BTreeMap<String, VariableDefinition> {
        &self.migration_config.variables
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
            template::RenderKind::Migration,
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
