use crate::commands::migration::get_pending_and_confirm;
use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::engine::MigrationError;
use anyhow::{anyhow, Result};

pub struct AdoptMigration {
    pub migration: Option<String>,
    pub yes: bool,
}

impl TelemetryDescribe for AdoptMigration {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("migration adopt")
    }
}

impl Command for AdoptMigration {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let migrations = match &self.migration {
            Some(migration) => vec![migration.clone()],
            None => match get_pending_and_confirm(config, "adopt", self.yes).await? {
                Some(pending) => pending,
                None => return Ok(Outcome::AdoptedMigration),
            },
        };

        let engine = config.new_engine().await?;

        for migration in &migrations {
            match engine
                .migration_adopt(migration, super::DEFAULT_NAMESPACE)
                .await
            {
                Ok(msg) => {
                    println!("{}", msg);
                }
                Err(MigrationError::AlreadyApplied { info, .. }) => {
                    println!(
                        "Migration '{}' already applied (status: {}, activity: {})",
                        migration, info.last_status, info.last_activity
                    );
                }
                Err(e) => {
                    return Err(
                        anyhow!(e).context(format!("Failed adopting migration '{}'", migration))
                    );
                }
            }
        }

        Ok(Outcome::AdoptedMigration)
    }
}
