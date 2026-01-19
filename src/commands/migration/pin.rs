use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::pinfile::LockData;
use crate::store::pinner::spawn::Spawn;
use crate::store::pinner::Pinner;
use anyhow::{Context, Result};

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
        let mut pinner = Spawn::new(
            config.pather().pinned_folder(),
            config.pather().components_folder(),
        )
        .context("could not get pinned_folder")?;

        let root = pinner
            .snapshot(config.operator())
            .await
            .context("error calling pinner snapshot")?;

        let lock_file_path = config.pather().migration_lock_file_path(&self.migration);
        let toml_str = toml::to_string_pretty(&LockData { pin: root.clone() })
            .context("could not not convert pin data to toml")?;

        config
            .operator()
            .write(&lock_file_path, toml_str)
            .await
            .context("failed writing migration lockfile")?;

        Ok(Outcome::PinnedMigration { hash: root })
    }
}
