//! Running a script directly against the database.
//!
//! Unlike a migration or test, a script is neither applied in order nor
//! diffed against a golden file, and lives outside the pinned component tree.

use crate::config;
use crate::template::{self, RenderKind};

use anyhow::{anyhow, Context, Result};

/// How a script chooses the component tree its includes resolve against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PinMode {
    /// Pinned if the script has a pin, otherwise the current components.
    #[default]
    Auto,
    /// Require the script's pin; an unpinned script is an error.
    Required,
    /// Ignore any pin and use the current components.
    Disabled,
}

/// A script addressed by its path relative to `scripts/`, e.g. `maintenance/reindex`.
#[derive(Debug)]
pub struct Script {
    config: config::Config,
    name: String,
    mode: PinMode,
    /// Resolve includes against this script's pinned component tree, not the live one.
    use_pinned: bool,
}

impl Script {
    pub fn new(config: &config::Config, name: &str, use_pinned: bool) -> Self {
        Script {
            config: config.clone(),
            name: name.to_string(),
            mode: if use_pinned {
                PinMode::Required
            } else {
                PinMode::Disabled
            },
            use_pinned,
        }
    }

    /// Builds a script with `mode` resolved against whether it has a pin on
    /// disk, so a pinned script is never silently run against other components.
    pub async fn resolve(config: &config::Config, name: &str, mode: PinMode) -> Result<Self> {
        let script = Script::new(config, name, false);
        let use_pinned = match mode {
            PinMode::Auto => script.is_pinned().await?,
            PinMode::Required if !script.is_pinned().await? => {
                return Err(anyhow!(
                    "script '{}' is not pinned: run `spawn script pin {}`, or drop --pinned",
                    name,
                    name
                ))
            }
            PinMode::Required => true,
            PinMode::Disabled => false,
        };
        Ok(Script {
            mode,
            use_pinned,
            ..script
        })
    }

    /// Whether includes resolve against the pinned tree.
    pub fn use_pinned(&self) -> bool {
        self.use_pinned
    }

    /// One line for stderr naming the tree the includes resolved against, so
    /// neither a run nor a build is ambiguous about which components it used.
    pub fn pin_note(&self) -> String {
        match (self.use_pinned, self.mode) {
            (true, _) => format!(
                "Using the pinned component tree for '{}' ({}).",
                self.name,
                self.config.pather().script_config_path(&self.name)
            ),
            (false, PinMode::Disabled) => format!(
                "Using the current component tree for '{}' (--no-pin).",
                self.name
            ),
            (false, _) => format!(
                "Using the current component tree for '{}' (not pinned).",
                self.name
            ),
        }
    }

    /// Whether this script's entry template exists on disk.
    ///
    /// Checked even for a pinned run: the entry lives in `scripts/`, which is
    /// never pinned.
    pub async fn exists(&self) -> Result<bool> {
        let path = self.config.pather().script_file_path(&self.name);
        self.config
            .operator()
            .exists(&path)
            .await
            .with_context(|| format!("failed to check for script '{}'", self.name))
    }

    /// Whether this script has been pinned.
    pub async fn is_pinned(&self) -> Result<bool> {
        let path = self.config.pather().script_config_path(&self.name);
        self.config
            .operator()
            .exists(&path)
            .await
            .with_context(|| format!("failed to check source config for script '{}'", self.name))
    }

    pub async fn generate_streaming(
        &self,
        variables: Option<crate::variables::Variables>,
        secrets_mode: crate::secrets::SecretsRenderMode,
    ) -> Result<template::StreamingGeneration> {
        let source_config_path = if self.use_pinned {
            Some(self.config.pather().script_config_path(&self.name))
        } else {
            None
        };

        template::generate_streaming(
            &self.config,
            source_config_path,
            &self.name,
            RenderKind::Script,
            &self.config.pather().script_file_path(&self.name),
            variables,
            secrets_mode,
        )
        .await
    }

    /// The pin hash an unpinned run should record: recomputed from the current
    /// component tree, `store_path: None` so nothing is written to the store.
    /// Mirrors [`crate::migrator::Migrator::recompute_pin_hash`].
    pub async fn recompute_pin_hash(&self) -> Result<String> {
        crate::store::pinner::snapshot(
            self.config.operator(),
            None,
            &self.config.pather().components_folder(),
        )
        .await
    }
}
