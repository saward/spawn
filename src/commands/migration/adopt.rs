use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::engine::MigrationError;
use anyhow::{anyhow, Result};

pub struct AdoptMigration {
    pub migration: String,
}

impl TelemetryDescribe for AdoptMigration {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("migration adopt")
    }
}

impl Command for AdoptMigration {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let engine = config.new_engine().await?;

        match engine.migration_adopt(&self.migration, "default").await {
            Ok(msg) => {
                println!("{}", msg);
                Ok(Outcome::AdoptedMigration)
            }
            Err(MigrationError::AlreadyApplied { info, .. }) => {
                println!(
                    "Migration '{}' already applied (status: {}, activity: {})",
                    &self.migration, info.last_status, info.last_activity
                );
                Ok(Outcome::AdoptedMigration)
            }
            Err(e) => {
                Err(anyhow!(e).context(format!("Failed adopting migration '{}'", &self.migration)))
            }
        }
    }
}
