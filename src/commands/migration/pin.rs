use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::source_config::write_pin;
use crate::store::pinner::spawn::Spawn;
use crate::store::pinner::Pinner;
use anyhow::{Context, Result};
use thiserror::Error;

/// Errors specific to pin operations
#[derive(Debug, Error)]
pub enum PinError {
    /// Migration folder does not exist
    #[error("migration folder '{0}' does not exist")]
    MigrationNotFound(String),
}

pub struct PinMigration {
    pub migration: String,
}

impl TelemetryDescribe for PinMigration {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("migration pin")
    }
}

impl Command for PinMigration {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        // Fail early if migration doesn't exist - check for the up.sql file which must exist:
        let migration_script = config.pather().migration_script_file_path(&self.migration);
        if !config
            .operator()
            .exists(&migration_script)
            .await
            .context("failed to check migration script")?
        {
            return Err(PinError::MigrationNotFound(self.migration.clone()).into());
        }

        let mut pinner = Spawn::new(
            config.pather().pinned_folder(),
            config.pather().components_folder(),
        )
        .context("could not get pinned_folder")?;

        let root = pinner
            .snapshot(config.operator())
            .await
            .context("error calling pinner snapshot")?;

        // Sets only the `pin` key: lock.toml may already declare variables,
        // which pinning has no business discarding or reformatting.
        let lock_file_path = config.pather().migration_lock_file_path(&self.migration);
        write_pin(config.operator(), &lock_file_path, &root)
            .await
            .context("failed writing migration lockfile")?;

        Ok(Outcome::PinnedMigration { hash: root })
    }
}
