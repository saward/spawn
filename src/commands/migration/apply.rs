use crate::commands::migration::get_pending_and_confirm;
use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::engine::{Engine, MigrationError};
use crate::migrator::Migrator;
use crate::secrets::{SecretsRenderMode, SecretsRepository};
use crate::variables::Variables;
use anyhow::{anyhow, Result};

/// Replaces every value this render actually resolved with a placeholder,
/// across the error's full chain, and rebuilds a flat error from the
/// result. `apply` executes real SQL, and a failing statement (e.g. a
/// constraint violation) can make Postgres echo a literal value — possibly
/// a secret — back in its own error output, entirely independent of the
/// migration_history checksum protection. See the Secrets guide for the
/// residual risk this doesn't close.
fn redact_secrets(err: anyhow::Error, secrets: &SecretsRepository) -> anyhow::Error {
    let mut text = format!("{:?}", err);
    // Longest first: if a shorter secret is a substring of a longer one,
    // redacting the shorter one first would consume part of the longer one
    // and leave the rest of it exposed.
    let mut values = secrets.resolved_values();
    values.sort_by_key(|b| std::cmp::Reverse(b.len()));
    for value in values {
        if value.is_empty() {
            continue;
        }
        text = text.replace(&value, "***REDACTED***");
    }
    anyhow!(text)
}

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
            // Apply actually executes against the database, so secrets must always be revealed.
            match mgrtr
                .generate_streaming(self.variables.clone(), SecretsRenderMode::Revealed)
                .await
            {
                Ok(streaming) => {
                    // Fingerprint the raw template source (never the rendered
                    // output, which may contain resolved secret values that
                    // could be guessed due to us using a fast hashing method)
                    // and the component tree it was built against, before the
                    // streaming generation is consumed below.
                    let checksum = streaming.raw_checksum();
                    let pin_hash = mgrtr.pin_hash().await?;

                    // Use shared engine if reuse_connection is enabled, otherwise create new
                    let new_engine: Option<Box<dyn Engine>>;
                    let engine: &dyn Engine = match &shared_engine {
                        Some(e) => e.as_ref(),
                        None => {
                            new_engine = Some(config.new_engine().await?);
                            new_engine.as_ref().unwrap().as_ref()
                        }
                    };
                    let (write_fn, secrets) = streaming.into_writer_fn();
                    match engine
                        .migration_apply(
                            &migration,
                            write_fn,
                            checksum,
                            Some(pin_hash),
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
                            let err =
                                e.context(format!("Failed applying migration {}", &migration));
                            return Err(redact_secrets(err, &secrets));
                        }
                        Err(MigrationError::AdvisoryLock(e)) => {
                            let err =
                                anyhow!("Unable to obtain advisory lock for migration").context(e);
                            return Err(redact_secrets(err, &secrets));
                        }
                        Err(e @ MigrationError::NotRecorded { .. }) => {
                            return Err(redact_secrets(anyhow!("{}", e), &secrets));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{SecretDefinition, SecretSource};
    use std::collections::HashMap;

    #[tokio::test]
    async fn redact_secrets_strips_resolved_values_from_the_full_error_chain() {
        let mut definitions = HashMap::new();
        definitions.insert(
            "application_password".to_string(),
            SecretDefinition {
                default: SecretSource::Literal {
                    value: "hunter2".to_string(),
                    insecure: true,
                },
                environments: HashMap::new(),
            },
        );
        let op = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
        let secrets = SecretsRepository::new(
            definitions,
            "prod".to_string(),
            SecretsRenderMode::Revealed,
            op,
        );
        // Resolve it, as a real render would while streaming to psql.
        secrets.resolve("application_password").await.unwrap();

        let simulated_psql_error = anyhow!(
            "psql exited with code 1: ERROR: duplicate key value\nDETAIL: Key (password)=(hunter2) already exists."
        );

        let redacted = redact_secrets(simulated_psql_error, &secrets);

        let full_text = format!("{:?}", redacted);
        assert!(!full_text.contains("hunter2"));
        assert!(full_text.contains("***REDACTED***"));
    }

    #[tokio::test]
    async fn redact_secrets_fully_redacts_a_secret_that_contains_another_as_a_substring() {
        // This test will only fail some of the time if the sort order is ever
        // changed to be random, but that's better than nothing.
        let mut definitions = HashMap::new();
        definitions.insert(
            "short".to_string(),
            SecretDefinition {
                default: SecretSource::Literal {
                    value: "hunter2".to_string(),
                    insecure: true,
                },
                environments: HashMap::new(),
            },
        );
        definitions.insert(
            "long".to_string(),
            SecretDefinition {
                default: SecretSource::Literal {
                    value: "hunter2suffix".to_string(),
                    insecure: true,
                },
                environments: HashMap::new(),
            },
        );
        let op = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
        let secrets = SecretsRepository::new(
            definitions,
            "prod".to_string(),
            SecretsRenderMode::Revealed,
            op,
        );
        secrets.resolve("short").await.unwrap();
        secrets.resolve("long").await.unwrap();

        let simulated_psql_error = anyhow!(
            "psql exited with code 1: ERROR: duplicate key value\nDETAIL: Key (password)=(hunter2suffix) already exists."
        );

        let redacted = redact_secrets(simulated_psql_error, &secrets);

        // Regardless of which order resolved_values() happens to hand back
        // the two secrets, the longer one must be redacted whole — not just
        // the "hunter2" prefix it shares with the shorter secret.
        let full_text = format!("{:?}", redacted);
        assert!(!full_text.contains("hunter2"), "got: {}", full_text);
        assert!(!full_text.contains("suffix"), "got: {}", full_text);
        assert!(full_text.contains("***REDACTED***"), "got: {}", full_text);
    }
}
