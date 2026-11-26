use crate::commands::migration::get_pending_and_confirm;
use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::engine::{Engine, MigrationError};
use crate::migrator::Migrator;
use crate::secrets::{redact_secrets, SecretsRenderMode};
use crate::variables::{resolve_declared, PromptMode, Variables};
use anyhow::{anyhow, Context, Result};

pub struct ApplyMigration {
    pub migration: Option<String>,
    pub pinned: bool,
    pub variables: Option<Variables>,
    pub yes: bool,
    pub retry: bool,
    pub reuse_connection: bool,
    /// Take declared defaults instead of asking, for unattended applies.
    pub use_defaults: bool,
}

impl TelemetryDescribe for ApplyMigration {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("migration apply").with_properties(vec![
            ("opt_pinned", self.pinned.to_string()),
            ("has_variables", self.variables.is_some().to_string()),
            ("apply_all", self.migration.is_none().to_string()),
            ("opt_reuse_connection", self.reuse_connection.to_string()),
            ("opt_use_defaults", self.use_defaults.to_string()),
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

        // The whole batch is prepared before any of it is applied, in two
        // passes, because each pass can refuse the batch and the cheaper one
        // should get to refuse first.
        //
        // Pass one: every pin. Nothing has been asked for or applied yet, so
        // an unpinned migration anywhere in the batch costs nothing to
        // discover. Reading each lock.toml here is also what pass two reads
        // its declarations from.
        let mut resolved: Vec<(String, Migrator)> = Vec::with_capacity(migrations.len());
        for migration in migrations {
            let mut mgrtr = Migrator::resolve(config, &migration, self.pinned).await;
            if self.pinned {
                mgrtr = mgrtr.with_context(|| {
                    format!(
                        "apply '{}' with --no-pin to use the current components",
                        migration
                    )
                });
            }
            let mgrtr = mgrtr?;
            resolved.push((migration, mgrtr));
        }

        // Pass two: every declared variable, which may mean prompting. Asking
        // inside the apply loop would strand a batch half-applied while a
        // later migration's prompt waited for an answer, and asking before
        // pass one would spend the answers on a batch that cannot run at all.
        // `--yes` means unattended, so it suppresses these as well as the
        // confirmation above.
        let mut pending: Vec<(String, Migrator, Option<Variables>)> =
            Vec::with_capacity(resolved.len());
        for (migration, mgrtr) in resolved {
            let declared = resolve_declared(
                mgrtr.declared_variables(),
                self.variables.as_ref(),
                PromptMode::resolve(self.use_defaults, self.yes),
            )
            .with_context(|| format!("migration '{}'", migration))?;
            let variables = Variables::with_defaults(self.variables.clone(), &declared)?;
            pending.push((migration, mgrtr, variables));
        }

        let total = pending.len();
        let dialect = crate::template::engine_to_dialect(&config.target_config()?.engine);

        // Optionally reuse the same engine (database connection) across all migrations
        let shared_engine = if self.reuse_connection {
            Some(config.new_engine().await?)
        } else {
            None
        };

        for (i, (migration, mgrtr, variables)) in pending.into_iter().enumerate() {
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
            // Apply actually executes against the database, so secrets must always be revealed.
            match mgrtr
                .generate_streaming(variables, SecretsRenderMode::Revealed)
                .await
            {
                Ok(streaming) => {
                    // Fingerprint the raw template source (never the rendered
                    // output, which may contain resolved secret values that
                    // could be guessed due to us using a fast hashing method)
                    // and the component tree it was built against, before the
                    // streaming generation is consumed below.
                    let checksum = streaming.raw_checksum();
                    // Prefer the pin this render actually used over a fresh
                    // re-read of lock.toml: a second, later read here could
                    // race a concurrent re-pin and record a hash that
                    // doesn't match what was rendered. Only --no-pin (no
                    // lock file involved) falls back to recomputing one.
                    let pin_hash = match streaming.pin_hash() {
                        Some(hash) => hash.to_string(),
                        None => mgrtr.recompute_pin_hash().await?,
                    };

                    // Use shared engine if reuse_connection is enabled, otherwise create new
                    let new_engine: Option<Box<dyn Engine>>;
                    let engine: &dyn Engine = match &shared_engine {
                        Some(e) => e.as_ref(),
                        None => {
                            new_engine = Some(config.new_engine().await?);
                            new_engine.as_ref().unwrap().as_ref()
                        }
                    };
                    let (script, secrets) = streaming.into_script_source();
                    match engine
                        .migration_apply(
                            &migration,
                            script,
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
                            return Err(redact_secrets(err, &secrets, dialect));
                        }
                        Err(MigrationError::AdvisoryLock(e)) => {
                            let err =
                                anyhow!("Unable to obtain advisory lock for migration").context(e);
                            return Err(redact_secrets(err, &secrets, dialect));
                        }
                        Err(e @ MigrationError::NotRecorded { .. }) => {
                            return Err(redact_secrets(anyhow!("{}", e), &secrets, dialect));
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
