use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::engine::{Engine, MigrationError, MigrationRevertError};
use crate::migrator::Migrator;
use crate::variables::Variables;
use anyhow::{anyhow, Result};

pub struct RevertMigration {
    pub migration: String,
    pub pinned: bool,
    pub variables: Option<Variables>,
    pub yes: bool,
    pub retry: bool,
}

impl TelemetryDescribe for RevertMigration {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("migration revert").with_properties(vec![
            ("opt_pinned", self.pinned.to_string()),
            ("has_variables", self.variables.is_some().to_string()),
        ])
    }
}

impl Command for RevertMigration {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let mgrtr = Migrator::new(config, &self.migration, self.pinned);

        match mgrtr.generate_streaming(self.variables.clone()).await {
            Ok(streaming) => {
                let new_engine: Option<Box<dyn Engine>>;
                let engine: &dyn Engine = {
                    new_engine = Some(config.new_engine().await?);
                    new_engine.as_ref().unwrap().as_ref()
                };
                let write_fn = streaming.into_writer_fn();
                match engine
                    .migration_revert(
                        &self.migration,
                        write_fn,
                        None,
                        super::DEFAULT_NAMESPACE,
                        self.retry,
                    )
                    .await
                {
                    Ok(_) => {
                        println!("Migration '{}' reverted successfully", &self.migration);
                    }
                    Err(MigrationRevertError::AlreadyReverted { info, .. }) => {
                        println!(
                            "Migration '{}' already reverted (status: {}, checksum: {})",
                            &self.migration, info.last_status, info.checksum
                        );
                    }
                    Err(MigrationRevertError::Common(common_error)) => match common_error {
                        MigrationError::PreviousAttemptFailed { status, info, .. } => {
                            return Err(anyhow!(
                                "Migration '{}' has a previous {} attempt (checksum: {}).\n\
                                         Use `spawn migration revert --retry {}` to retry.",
                                &self.migration,
                                status,
                                info.checksum,
                                &self.migration,
                            ));
                        }

                        MigrationError::Database(e) => {
                            return Err(e.context(format!(
                                "Failed reverting migration {}",
                                &self.migration,
                            )));
                        }

                        MigrationError::AdvisoryLock(e) => {
                            return Err(
                                anyhow!("Unable to obtain advisory lock for migration").context(e)
                            );
                        }

                        e @ MigrationError::NotRecorded { .. } => {
                            return Err(anyhow!("{e}"));
                        }
                    },
                }
            }
            Err(e) => {
                let context = if self.pinned {
                    anyhow!(
                            "Failed to generate migration '{}'. Is it pinned? \
                             Run `spawn migration pin {}` or use `--no-pin` to revert without pinning.",
                            &self.migration, &self.migration
                        )
                } else {
                    anyhow!("failed to generate migration '{}'", &self.migration)
                };
                return Err(e.context(context));
            }
        };
        Ok(Outcome::RevertedMigration)
    }
}
