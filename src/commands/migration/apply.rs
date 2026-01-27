use crate::commands::migration::get_pending_and_confirm;
use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::engine::MigrationError;
use crate::migrator::Migrator;
use crate::variables::Variables;
use anyhow::{anyhow, Result};

pub struct ApplyMigration {
    pub migration: Option<String>,
    pub pinned: bool,
    pub variables: Option<Variables>,
    pub yes: bool,
}

impl TelemetryDescribe for ApplyMigration {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("migration apply").with_properties(vec![
            ("opt_pinned", self.pinned.to_string()),
            ("has_variables", self.variables.is_some().to_string()),
            ("apply_all", self.migration.is_none().to_string()),
        ])
    }
}

impl Command for ApplyMigration {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let migrations = match &self.migration {
            Some(migration) => vec![migration.clone()],
            None => match get_pending_and_confirm(config, "apply", self.yes).await? {
                Some(pending) => pending,
                None => return Ok(Outcome::AppliedMigrations),
            },
        };

        for migration in migrations {
            let mgrtr = Migrator::new(config, &migration, self.pinned);
            match mgrtr.generate_streaming(self.variables.clone()).await {
                Ok(streaming) => {
                    let engine = config.new_engine().await?;
                    let write_fn = streaming.into_writer_fn();
                    match engine
                        .migration_apply(&migration, write_fn, None, super::DEFAULT_NAMESPACE)
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
                                e.context(format!("Failed applying migration {}", &migration))
                            );
                        }
                        Err(MigrationError::AdvisoryLock(e)) => {
                            return Err(
                                anyhow!("Unable to obtain advisory lock for migration").context(e)
                            );
                        }
                        Err(e @ MigrationError::NotRecorded { .. }) => {
                            return Err(anyhow!("{}", e));
                        }
                    }
                }
                Err(e) => {
                    let context = if self.pinned {
                        anyhow!(
                            "Failed to generate migration '{}'. Is it pinned? \
                             Run `spawn migration pin {}` or use `--no-pin` to apply without pinning.",
                            &migration, &migration
                        )
                    } else {
                        anyhow!("failed to generate migration '{}'", &migration)
                    };
                    return Err(e.context(context));
                }
            };
        }
        Ok(Outcome::AppliedMigrations)
    }
}
