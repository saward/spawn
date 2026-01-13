// This is a driver that uses a locally provided PSQL command to execute
// scripts, which enables user's scripts to take advantage of things like the
// build in PSQL helper commands.

use crate::config::FolderPather;
use crate::engine::{
    DatabaseConfig, Engine, EngineOutputter, EngineWriter, ExistingMigrationInfo, MigrationError,
    MigrationHistoryStatus, MigrationResult,
};
use crate::escape::{EscapedIdentifier, EscapedLiteral, EscapedQuery, InsecureRawSql};
use crate::sql_query;
use crate::store::pinner::latest::Latest;
use crate::store::{operator_from_includedir, Store};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use include_dir::{include_dir, Dir};
use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::Instant;
use twox_hash::xxhash3_128;
use twox_hash::XxHash64;

#[derive(Debug)]
pub struct PSQL {
    psql_command: Vec<String>,
    /// Schema name as an escaped identifier (for use as schema.table)
    spawn_schema_ident: EscapedIdentifier,
    /// Schema name as an escaped literal (for use in WHERE clauses)
    spawn_schema_literal: EscapedLiteral,
}

static PROJECT_DIR: Dir<'_> = include_dir!("./static/engine-migrations/postgres-psql");
static SPAWN_NAMESPACE: &'static str = "spawn";

pub struct PSQLWriter {
    child: Child,
    stdin: ChildStdin,
}

pub struct PSQLOutput {
    child: Child,
}

impl PSQL {
    pub fn new(config: &DatabaseConfig) -> Result<Box<dyn Engine>> {
        let psql_command = config
            .command
            .clone()
            .ok_or(anyhow!("Command for database config must be defined"))?;

        // Use type-safe escaped types - escaping happens at construction time
        let spawn_schema_ident = EscapedIdentifier::new(&config.spawn_schema);
        let spawn_schema_literal = EscapedLiteral::new(&config.spawn_schema);

        Ok(Box::new(Self {
            psql_command,
            spawn_schema_ident,
            spawn_schema_literal,
        }))
    }

    fn safe_spawn_namespace(&self) -> EscapedLiteral {
        EscapedLiteral::new(SPAWN_NAMESPACE)
    }
}

#[async_trait]
impl Engine for PSQL {
    fn new_writer(&self) -> Result<Box<dyn EngineWriter>> {
        let mut parts = self.psql_command.clone();
        let command = parts.remove(0);
        let mut child = &mut Command::new(command);
        for arg in parts {
            child = child.arg(arg);
        }
        let mut child = child
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit()) // Let stderr go to terminal so users see errors
            .spawn()
            .expect("failed to execute command");

        let stdin = child
            .stdin
            .take()
            .ok_or(anyhow::anyhow!("no stdin found"))?;

        Ok(Box::new(PSQLWriter { child, stdin }) as Box<dyn EngineWriter>)
    }

    async fn migration_apply(
        &self,
        migration_name: &str,
        migration: &str,
        pin_hash: Option<String>,
        namespace: &str,
    ) -> MigrationResult<String> {
        // Ensure we have latest schema:
        self.update_schema()
            .await
            .map_err(MigrationError::Database)?;
        self.apply_and_record_migration_v1(
            migration_name,
            migration,
            pin_hash,
            EscapedLiteral::new(namespace),
        )
    }
}

impl PSQL {
    pub async fn update_schema(&self) -> Result<()> {
        // Create a memory operator from the included directory containing
        // the engine's own migration scripts
        let op = operator_from_includedir(&PROJECT_DIR, None)
            .await
            .context("Failed to create operator from included directory")?;

        // Create a pinner and store to list and load migrations
        let pinner = Latest::new("").context("Failed to create Latest pinner")?;
        let pather = FolderPather {
            spawn_folder: "".to_string(),
        };
        let store = Store::new(Box::new(pinner), op, pather)
            .context("Failed to create store for update_schema")?;

        // Get list of all available migrations (sorted oldest to newest)
        let available_migrations = store
            .list_migrations()
            .await
            .context("Failed to list migrations")?;

        // Check if migration table exists to determine if this is bootstrap
        let migration_table_exists = self.migration_table_exists()?;

        // Get set of already applied migrations (empty if table doesn't exist)
        let applied_migrations: HashSet<String> = if migration_table_exists {
            self.get_applied_migrations_set(&self.safe_spawn_namespace())?
        } else {
            HashSet::new()
        };

        // Apply each migration that hasn't been applied yet
        for migration_path in available_migrations {
            // Extract migration name from path (e.g., "migrations/001-base-migration-table/" -> "001-base-migration-table")
            let migration_name = migration_path
                .trim_end_matches('/')
                .rsplit('/')
                .next()
                .unwrap_or(&migration_path);

            // Skip if already applied
            if applied_migrations.contains(migration_name) {
                continue;
            }

            // Load and render the migration
            let up_sql_path = format!("{}up.sql", migration_path);
            // Load the raw migration SQL
            let migration_sql = store
                .load_migration(&up_sql_path)
                .await
                .context(format!("Failed to load migration {}", migration_name))?;

            // Render the template with variables (type-safe escaped identifier)
            let rendered_sql =
                migration_sql.replace("{{schema}}", self.spawn_schema_ident.as_str());

            // Apply the migration and record it
            // Note: even for bootstrap, the first migration creates the tables,
            // so they exist by the time we record the migration.
            match self.apply_and_record_migration_v1(
                migration_name,
                &rendered_sql,
                None, // pin_hash not used for engine migrations
                self.safe_spawn_namespace(),
            ) {
                Ok(_) => {}
                // For internal schema migrations, already applied is fine
                Err(MigrationError::AlreadyApplied { .. }) => {}
                // Other errors should propagate
                Err(e) => return Err(e.into()),
            }
        }

        Ok(())
    }

    fn execute_sql(&self, query: &EscapedQuery, format: Option<&str>) -> Result<String> {
        let mut writer = self.new_writer()?;

        self.execute_sql_part(query, format, &mut writer)?;
        let mut outputter = writer.finalise()?;
        let output = outputter.output()?;
        // Error if we can't read:
        let output = String::from_utf8(output)?;
        Ok(output)
    }

    fn execute_sql_part(
        &self,
        query: &EscapedQuery,
        format: Option<&str>,
        writer: &mut Box<dyn EngineWriter>,
    ) -> Result<()> {
        // Make psql exit with non-zero status on SQL errors
        writer.write_all(b"\\set ON_ERROR_STOP on\n")?;
        // Assumes psql_command already connects to the correct database
        if let Some(format) = format {
            // tuples_only suppresses headers and extra psql messages
            writer.write_all(b"\\set QUIET on\n")?;
            writer.write_all(b"\\pset tuples_only on\n")?;
            writer.write_all(format!("\\pset format {}\n", format).as_bytes())?;
        }
        Ok(writer.write_all(query.as_str().as_bytes())?)
    }

    fn migration_table_exists(&self) -> Result<bool> {
        self.table_exists("migration")
    }

    fn migration_history_table_exists(&self) -> Result<bool> {
        self.table_exists("migration_history")
    }

    fn table_exists(&self, table_name: &str) -> Result<bool> {
        let safe_table_name = EscapedLiteral::new(table_name);
        let query = sql_query!(
            r#"
            SELECT EXISTS (
                SELECT FROM information_schema.tables
                WHERE table_schema = {}
                AND table_name = {}
            );
            "#,
            self.spawn_schema_literal,
            safe_table_name
        );

        let output = self.execute_sql(&query, Some("csv"))?;
        // With tuples_only mode, output is just "t" or "f"
        Ok(output.trim() == "t")
    }

    fn get_applied_migrations_set(&self, namespace: &EscapedLiteral) -> Result<HashSet<String>> {
        let query = sql_query!(
            "SELECT name FROM {}.migration WHERE namespace = {};",
            self.spawn_schema_ident,
            namespace,
        );

        let output = self.execute_sql(&query, Some("csv"))?;
        let mut migrations = HashSet::new();

        // With tuples_only mode, we get just the data rows (no headers)
        for line in output.lines() {
            let name = line.trim();
            if !name.is_empty() {
                migrations.insert(name.to_string());
            }
        }

        Ok(migrations)
    }

    /// Get the latest migration history entry for a given migration name and namespace.
    /// Returns None if no history entry exists.
    fn get_migration_status(
        &self,
        migration_name: &str,
        namespace: &EscapedLiteral,
    ) -> Result<Option<ExistingMigrationInfo>> {
        let safe_migration_name = EscapedLiteral::new(migration_name);
        let query = sql_query!(
            r#"
            SELECT m.name, m.namespace, mh.status_id_status, mh.activity_id_activity, encode(mh.checksum, 'hex')
            FROM {}.migration_history mh
            JOIN {}.migration m ON mh.migration_id_migration = m.migration_id
            WHERE m.name = {} AND m.namespace = {}
            ORDER BY mh.migration_history_id DESC
            LIMIT 1;
            "#,
            self.spawn_schema_ident,
            self.spawn_schema_ident,
            safe_migration_name,
            namespace
        );

        let output = self.execute_sql(&query, Some("csv"))?;

        // With tuples_only mode, we get just the data row (no headers).
        // Parse CSV: name,namespace,status_id_status,activity_id_activity,checksum
        let data_line = output.trim();
        if data_line.is_empty() {
            return Ok(None);
        }

        let parts: Vec<&str> = data_line.split(',').collect();
        if parts.len() < 5 {
            return Ok(None);
        }

        let status = match parts[2].trim() {
            "SUCCESS" => MigrationHistoryStatus::Success,
            "ATTEMPTED" => MigrationHistoryStatus::Attempted,
            "FAILURE" => MigrationHistoryStatus::Failure,
            _ => return Ok(None),
        };

        Ok(Some(ExistingMigrationInfo {
            migration_name: parts[0].trim().to_string(),
            namespace: parts[1].trim().to_string(),
            last_status: status,
            last_activity: parts[3].trim().to_string(),
            checksum: parts[4].trim().to_string(),
        }))
    }

    // This is versioned because if we change the schema significantly enough
    // later, we'll have to still write earlier migrations to the table using
    // the format of the migration table as it is at that point.
    fn apply_and_record_migration_v1(
        &self,
        migration_name: &str,
        migration_sql: &str,
        pin_hash: Option<String>,
        namespace: EscapedLiteral,
    ) -> MigrationResult<String> {
        // Check if migration already exists in history (skip if table doesn't exist yet)
        let existing_status = if self
            .migration_history_table_exists()
            .map_err(MigrationError::Database)?
        {
            self.get_migration_status(migration_name, &namespace)
                .map_err(MigrationError::Database)?
        } else {
            None
        };

        if let Some(info) = existing_status {
            let name = migration_name.to_string();
            let ns = namespace.raw_value().to_string();

            match info.last_status {
                MigrationHistoryStatus::Success => {
                    return Err(MigrationError::AlreadyApplied {
                        name,
                        namespace: ns,
                        info,
                    });
                }
                MigrationHistoryStatus::Attempted | MigrationHistoryStatus::Failure => {
                    return Err(MigrationError::PreviousAttemptFailed {
                        name,
                        namespace: ns,
                        status: info.last_status.clone(),
                        info,
                    });
                }
            }
        }

        // Record duration of execute_sql:
        let start_time = Instant::now();
        // We need to trust that the migration_sql is already safely escaped
        // for now:
        let migration_sql = sql_query!("{}", InsecureRawSql::new(migration_sql));

        let mut writer = self.new_writer()?;
        let checksum = XxHash64::oneshot(1234, "SPAWN_MIGRATION_LOCK".as_bytes()) as i64;
        writer
            .write_all(b"\\set ON_ERROR_STOP on\n")
            .map_err(|e| MigrationError::Database(e.into()))?;
        writer
            .write_all(
                format!(r#"DO $$ BEGIN IF NOT pg_try_advisory_lock({}) THEN RAISE EXCEPTION 'Could not acquire advisory lock'; END IF; END $$;"#, checksum)
                    .as_str()
                    .as_bytes(),
            )
            .map_err(|e| MigrationError::AdvisoryLock(e))?;

        self.execute_sql_part(&migration_sql, None, &mut writer)
            .map_err(MigrationError::Database)?;
        let duration = start_time.elapsed().as_secs_f32();

        let checksum = xxhash3_128::Hasher::oneshot(migration_sql.as_str().as_bytes());

        // Use type-safe escaped literals for dynamic values
        let safe_migration_name = EscapedLiteral::new(migration_name);
        let safe_checksum = EscapedLiteral::new(&format!("{:032x}", checksum));
        let safe_pin_hash = pin_hash.map(|hash| EscapedLiteral::new(&hash));

        // Record the migration (schema is pre-escaped in struct)
        // Use CTEs to chain the inserts and return counts for verification.
        // This avoids a separate SELECT that could silently return no rows.
        let duration_interval = InsecureRawSql::new(&format!("INTERVAL '{} second'", duration));
        let record_query = sql_query!(
            r#"
BEGIN;
SELECT 'begin_insertion_record';
WITH inserted_migration AS (
    INSERT INTO {}.migration (name, namespace) VALUES ({}, {})
    RETURNING migration_id
),
inserted_history AS (
    INSERT INTO {}.migration_history (
        migration_id_migration,
        activity_id_activity,
        created_by,
        description,
        status_note,
        status_id_status,
        checksum,
        execution_time,
        pin_hash
    )
    SELECT
        migration_id,
        'APPLY',
        'unused',
        '',
        '',
        'SUCCESS',
        {},
        {},
        {}
    FROM inserted_migration
    RETURNING migration_history_id
)
SELECT
    (SELECT count(*) FROM inserted_migration) as migration_count,
    (SELECT count(*) FROM inserted_history) as history_count;
COMMIT;
"#,
            self.spawn_schema_ident,
            safe_migration_name,
            namespace,
            self.spawn_schema_ident,
            safe_checksum,
            duration_interval,
            safe_pin_hash,
        );

        // Use CSV format for parseable output
        // CRITICAL: If this fails, the migration was applied but not recorded!
        let output = {
            self.execute_sql_part(&record_query, Some("csv"), &mut writer)
                .map_err(|e| MigrationError::MigrationAppliedButNotRecorded {
                    name: migration_name.to_string(),
                    namespace: namespace.raw_value().to_string(),
                    schema: self.spawn_schema_ident.raw_value().to_string(),
                    recording_error: e.to_string(),
                })?;

            let mut outputter = writer.finalise()?;
            let output =
                outputter
                    .output()
                    .map_err(|e| MigrationError::MigrationAppliedButNotRecorded {
                        name: migration_name.to_string(),
                        namespace: namespace.raw_value().to_string(),
                        schema: self.spawn_schema_ident.raw_value().to_string(),
                        recording_error: e.to_string(),
                    })?;

            // Error if we can't read:
            let output =
                String::from_utf8(output).map_err(|e| MigrationError::Database(e.into()))?;
            output
        };

        // With QUIET mode and tuples_only, CSV output is just "1,1" for successful inserts
        // CRITICAL: If this check fails, the migration was applied but recording may be incomplete!
        if !output.contains("begin_insertion_record\n1,1") {
            return Err(MigrationError::MigrationAppliedButNotRecorded {
                name: migration_name.to_string(),
                namespace: namespace.raw_value().to_string(),
                schema: self.spawn_schema_ident.raw_value().to_string(),
                recording_error: format!(
                    "expected 1 row inserted for both migration and history, got output: {}",
                    output
                ),
            });
        }

        Ok(output)
    }
}

impl crate::engine::EngineOutputter for PSQLOutput {
    fn output(&mut self) -> io::Result<Vec<u8>> {
        // Collect stdout
        let mut stdout = Vec::new();
        if let Some(mut out) = self.child.stdout.take() {
            out.read_to_end(&mut stdout)?;
        }

        // Wait for the child and check exit status
        // (stderr goes directly to terminal via Stdio::inherit)
        let status = self.child.wait()?;

        if !status.success() {
            let stdout_str = String::from_utf8_lossy(&stdout);
            let error_msg = format!("psql exited with status {}: {}", status, stdout_str.trim());
            return Err(io::Error::new(io::ErrorKind::Other, error_msg));
        }

        Ok(stdout)
    }
}

impl crate::engine::EngineWriter for PSQLWriter {
    fn finalise(mut self: Box<Self>) -> Result<Box<dyn EngineOutputter>> {
        // Ensure writing is finished (not sure if necessary):
        self.flush()?;
        Ok(Box::new(PSQLOutput { child: self.child }))
    }
}

impl io::Write for PSQLWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stdin.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stdin.flush()
    }
}
