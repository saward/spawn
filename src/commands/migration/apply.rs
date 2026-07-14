use crate::commands::migration::get_pending_and_confirm;
use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::engine::{Engine, MigrationError};
use crate::migrator::Migrator;
use crate::variables::Variables;
use anyhow::{anyhow, Result};

pub struct ApplyMigration {
    pub migration: Option<String>,
    pub pinned: bool,
    pub variables: Option<Variables>,
    pub yes: bool,
    pub retry: bool,
    pub reuse_connection: bool,
}

impl TelemetryDescribe for ApplyMigration {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("migration apply").with_properties(vec![
            ("opt_pinned", self.pinned.to_string()),
            ("has_variables", self.variables.is_some().to_string()),
            ("apply_all", self.migration.is_none().to_string()),
            ("opt_reuse_connection", self.reuse_connection.to_string()),
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

        let total = migrations.len();

        // Optionally reuse the same engine (database connection) across all migrations
        let shared_engine = if self.reuse_connection {
            Some(config.new_engine().await?)
        } else {
            None
        };

        for (i, migration) in migrations.into_iter().enumerate() {
            let counter = if total > 1 {
                format!(
                    "[{:>width$}/{}] ",
                    i + 1,
                    total,
                    width = total.to_string().len()
                )
            } else {
                String::new()
            };
            let mgrtr = Migrator::new(config, &migration, self.pinned);
            match mgrtr.generate_streaming(self.variables.clone()).await {
                Ok(streaming) => {
                    // Use shared engine if reuse_connection is enabled, otherwise create new
                    let new_engine: Option<Box<dyn Engine>>;
                    let engine: &dyn Engine = match &shared_engine {
                        Some(e) => e.as_ref(),
                        None => {
                            new_engine = Some(config.new_engine().await?);
                            new_engine.as_ref().unwrap().as_ref()
                        }
                    };
                    let write_fn = streaming.into_writer_fn();
                    match engine
                        .migration_apply(
                            &migration,
                            write_fn,
                            None,
                            super::DEFAULT_NAMESPACE,
                            self.retry,
                        )
                        .await
                    {
                        Ok(_) => {
                            println!("{}Migration '{}' applied successfully", counter, &migration);
                        }
                        Err(MigrationError::AlreadyApplied { info, .. }) => {
                            println!(
                                "{}Migration '{}' already applied (status: {}, checksum: {})",
                                counter, &migration, info.last_status, info.checksum
                            );
                        }
                        Err(MigrationError::PreviousAttemptFailed { status, info, .. }) => {
                            return Err(anyhow!(
                                "Migration '{}' has a previous {} attempt (checksum: {}).\n\
                                 Use `spawn migration apply --retry {}` to retry.",
                                &migration,
                                status,
                                info.checksum,
                                &migration,
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
