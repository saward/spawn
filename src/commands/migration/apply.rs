use crate::commands::{Command, Outcome, TelemetryDescribe};
use crate::config::Config;
use crate::engine::MigrationError;
use crate::migrator::Migrator;
use crate::variables::Variables;
use anyhow::{anyhow, Result};

pub struct ApplyMigration {
    pub migration: Option<String>,
    pub pinned: bool,
    pub variables: Option<Variables>,
}

impl TelemetryDescribe for ApplyMigration {
    fn telemetry_command(&self) -> String {
        "migration apply".to_string()
    }

    fn telemetry_properties(&self) -> Vec<(&'static str, String)> {
        vec![
            ("opt_pinned", self.pinned.to_string()),
            ("has_variables", self.variables.is_some().to_string()),
            ("apply_all", self.migration.is_none().to_string()),
        ]
    }
}

impl Command for ApplyMigration {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let mut migrations = Vec::new();
        match &self.migration {
            Some(migration) => migrations.push(migration.clone()),
            None => return Err(anyhow!("applying all migrations not implemented")),
        }

        for migration in migrations {
            let mgrtr = Migrator::new(config, &migration, self.pinned);
            match mgrtr.generate_streaming(self.variables.clone()).await {
                Ok(streaming) => {
                    let engine = config.new_engine().await?;
                    let write_fn = streaming.into_writer_fn();
                    match engine
                        .migration_apply(&migration, write_fn, None, "default")
                        .await
                    {
                        Ok(_) => {
                            println!("Migration '{}' applied successfully", &migration);
                        }
                        Err(MigrationError::AlreadyApplied { info, .. }) => {
                            println!(
                                "Migration '{}' already applied (status: {}, checksum: {})",
                                &migration, info.last_status, info.checksum
                            );
                        }
                        Err(MigrationError::PreviousAttemptFailed { status, info, .. }) => {
                            return Err(anyhow!(
                                "Migration '{}' has a previous {} attempt (checksum: {}). \
                                 Manual intervention may be required.",
                                &migration,
                                status,
                                info.checksum
                            ));
                        }
                        Err(MigrationError::Database(e)) => {
                            return Err(
                                anyhow!("Failed applying migration {}", &migration).context(e)
                            );
                        }
                        Err(MigrationError::AdvisoryLock(e)) => {
                            return Err(
                                anyhow!("Unable to obtain advisory lock for migration").context(e)
                            );
                        }
                        Err(e @ MigrationError::MigrationAppliedButNotRecorded { .. }) => {
                            return Err(anyhow!("{}", e));
                        }
                    }
                }
                Err(e) => {
                    return Err(e.context(anyhow!("failed to generate migration '{}'", &migration,)))
                }
            };
        }
        Ok(Outcome::AppliedMigrations)
    }
}
