// This is a driver that uses a locally provided PSQL command to execute
// scripts, which enables user's scripts to take advantage of things like the
// build in PSQL helper commands.

use crate::config::FolderPather;
use crate::engine::{
    DatabaseConfig, Engine, EngineError, ExistingMigrationInfo, MigrationError,
    MigrationHistoryStatus, MigrationResult, StdoutWriter, WriterFn,
};
use crate::escape::{EscapedIdentifier, EscapedLiteral, EscapedQuery, InsecureRawSql};
use crate::sql_query;
use crate::store::pinner::latest::Latest;
use crate::store::{operator_from_includedir, Store};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use include_dir::{include_dir, Dir};
use std::collections::HashSet;
use std::io::Write;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
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
static SPAWN_NAMESPACE: &str = "spawn";

impl PSQL {
    pub async fn new(config: &DatabaseConfig) -> Result<Box<dyn Engine>> {
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
    async fn execute_with_writer(
        &self,
        write_fn: WriterFn,
        stdout_writer: StdoutWriter,
    ) -> Result<(), EngineError> {
        // 1. Create the pipe for stdin
        let (reader, mut writer) = std::io::pipe()?;

        // 2. Configure stdout based on whether we have a writer
        let stdout_config = if stdout_writer.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        };

        // 3. Spawn psql reading from the pipe
        let mut child = Command::new(&self.psql_command[0])
            .args(&self.psql_command[1..])
            .stdin(Stdio::from(reader))
            .stdout(stdout_config)
            .stderr(Stdio::piped())
            .spawn()
            .map_err(EngineError::Io)?;

        // 4. If we have a stdout writer, spawn task to copy stdout to it
        let stdout_handle = if let Some(stdout_dest) = stdout_writer {
            let mut stdout = child.stdout.take().expect("stdout should be piped");
            Some(tokio::spawn(async move {
                let mut buf = Vec::new();
                let _ = stdout.read_to_end(&mut buf).await;
                // Write to the provided writer (in blocking context since Write is sync)
                let result = tokio::task::spawn_blocking(move || {
                    let mut dest = stdout_dest;
                    dest.write_all(&buf)
                })
                .await;
                result
            }))
        } else {
            None
        };

        // 5. Drain stderr in background (always, prevents deadlock)
        let mut stderr = child.stderr.take().expect("stderr should be piped");
        let stderr_handle = tokio::spawn(async move {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf).await;
            buf
        });

        // 6. Run the writer function in a blocking thread
        let writer_handle = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            // PSQL-specific setup - QUIET must be first to suppress output from other settings
            writer.write_all(b"\\set QUIET on\n")?;
            writer.write_all(b"\\pset pager off\n")?;
            writer.write_all(b"\\set ON_ERROR_STOP on\n")?;

            // User's write function (template rendering, etc.)
            write_fn(&mut writer)?;

            // Writer dropped here -> EOF to psql
            Ok(())
        });

        // 7. Wait for writing to complete
        writer_handle
            .await
            .map_err(|e| EngineError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))??;

        // 8. Wait for stdout copy if applicable (must complete before we read the buffer)
        if let Some(handle) = stdout_handle {
            // Wait for both the async read and the blocking write to complete
            let _ = handle.await;
        }

        // 9. Wait for psql and check result
        let status = child.wait().await?;
        let stderr_bytes = stderr_handle.await.unwrap_or_default();

        if !status.success() {
            return Err(EngineError::ExecutionFailed {
                exit_code: status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&stderr_bytes).to_string(),
            });
        }

        Ok(())
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
        .await
    }
}

/// A simple Write implementation that appends to a shared Vec<u8>
struct SharedBufWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedBufWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
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
        let migration_table_exists = self.migration_table_exists().await?;

        // Get set of already applied migrations (empty if table doesn't exist)
        let applied_migrations: HashSet<String> = if migration_table_exists {
            self.get_applied_migrations_set(&self.safe_spawn_namespace())
                .await?
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
            match self
                .apply_and_record_migration_v1(
                    migration_name,
                    &rendered_sql,
                    None, // pin_hash not used for engine migrations
                    self.safe_spawn_namespace(),
                )
                .await
            {
                Ok(_) => {}
                // For internal schema migrations, already applied is fine
                Err(MigrationError::AlreadyApplied { .. }) => {}
                // Other errors should propagate
                Err(e) => return Err(e.into()),
            }
        }

        Ok(())
    }

    /// Execute SQL and return stdout as a String.
    /// Used for internal queries where we need to parse results.
    async fn execute_sql(&self, query: &EscapedQuery, format: Option<&str>) -> Result<String> {
        let query_str = query.as_str().to_string();
        let format_owned = format.map(|s| s.to_string());

        // Create a shared buffer to capture stdout
        let stdout_buf = Arc::new(Mutex::new(Vec::new()));
        let stdout_buf_clone = stdout_buf.clone();

        self.execute_with_writer(
            Box::new(move |writer| {
                // Format settings if requested (QUIET is already set globally)
                if let Some(fmt) = format_owned {
                    writer.write_all(b"\\pset tuples_only on\n")?;
                    writer.write_all(format!("\\pset format {}\n", fmt).as_bytes())?;
                }
                writer.write_all(query_str.as_bytes())?;
                Ok(())
            }),
            Some(Box::new(SharedBufWriter(stdout_buf_clone))),
        )
        .await
        .map_err(|e| anyhow!("SQL execution failed: {}", e))?;

        let buf = stdout_buf.lock().unwrap();
        Ok(String::from_utf8_lossy(&buf).to_string())
    }

    async fn migration_table_exists(&self) -> Result<bool> {
        self.table_exists("migration").await
    }

    async fn migration_history_table_exists(&self) -> Result<bool> {
        self.table_exists("migration_history").await
    }

    async fn table_exists(&self, table_name: &str) -> Result<bool> {
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

        let output = self.execute_sql(&query, Some("csv")).await?;
        // With tuples_only mode, output is just "t" or "f"
        Ok(output.trim() == "t")
    }

    async fn get_applied_migrations_set(
        &self,
        namespace: &EscapedLiteral,
    ) -> Result<HashSet<String>> {
        let query = sql_query!(
            "SELECT name FROM {}.migration WHERE namespace = {};",
            self.spawn_schema_ident,
            namespace,
        );

        let output = self.execute_sql(&query, Some("csv")).await?;
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
    async fn get_migration_status(
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

        let output = self.execute_sql(&query, Some("csv")).await?;

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
    async fn apply_and_record_migration_v1(
        &self,
        migration_name: &str,
        migration_sql: &str,
        pin_hash: Option<String>,
        namespace: EscapedLiteral,
    ) -> MigrationResult<String> {
        // Check if migration already exists in history (skip if table doesn't exist yet)
        let existing_status = if self
            .migration_history_table_exists()
            .await
            .map_err(MigrationError::Database)?
        {
            self.get_migration_status(migration_name, &namespace)
                .await
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

        // We need to trust that the migration_sql is already safely escaped for now
        let migration_sql_query = sql_query!("{}", InsecureRawSql::new(migration_sql));
        let lock_checksum = XxHash64::oneshot(1234, "SPAWN_MIGRATION_LOCK".as_bytes()) as i64;

        // Calculate content checksum before execution
        let content_checksum = xxhash3_128::Hasher::oneshot(migration_sql.as_bytes());

        // Prepare values for the closure
        let migration_sql_str = migration_sql_query.as_str().to_string();
        let schema_ident = self.spawn_schema_ident.clone();
        let safe_migration_name = EscapedLiteral::new(migration_name);
        let safe_checksum = EscapedLiteral::new(&format!("{:032x}", content_checksum));
        let safe_pin_hash = pin_hash.map(|h| EscapedLiteral::new(&h));
        let namespace_clone = namespace.clone();
        let migration_name_owned = migration_name.to_string();
        let schema_ident_raw = self.spawn_schema_ident.raw_value().to_string();
        let namespace_raw = namespace.raw_value().to_string();

        // Execute migration and record it in a single psql session
        // No stdout capture needed for migrations
        let result = self
            .execute_with_writer(
                Box::new(move |writer| {
                    // Acquire advisory lock
                    writer.write_all(
                        format!(
                            r#"DO $$ BEGIN IF NOT pg_try_advisory_lock({}) THEN RAISE EXCEPTION 'Could not acquire advisory lock'; END IF; END $$;"#,
                            lock_checksum
                        )
                        .as_bytes(),
                    )?;

                    // Execute the migration SQL
                    writer.write_all(migration_sql_str.as_bytes())?;

                    // Calculate duration (approximate, since we're in the closure)
                    let duration = start_time.elapsed().as_secs_f32();
                    let duration_interval =
                        InsecureRawSql::new(&format!("INTERVAL '{} second'", duration));

                    // Record the migration
                    let record_query = sql_query!(
                        r#"
BEGIN;
WITH inserted_migration AS (
    INSERT INTO {}.migration (name, namespace) VALUES ({}, {})
    RETURNING migration_id
)
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
FROM inserted_migration;
COMMIT;
"#,
                        schema_ident,
                        safe_migration_name,
                        namespace_clone,
                        schema_ident,
                        safe_checksum,
                        duration_interval,
                        safe_pin_hash,
                    );

                    writer.write_all(record_query.as_str().as_bytes())?;

                    Ok(())
                }),
                None, // No stdout capture for migrations
            )
            .await;

        match result {
            Ok(()) => Ok("Migration applied successfully".to_string()),
            Err(EngineError::ExecutionFailed { exit_code, stderr }) => {
                // Check if the error is from advisory lock
                if stderr.contains("Could not acquire advisory lock") {
                    Err(MigrationError::AdvisoryLock(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        stderr,
                    )))
                } else {
                    // Migration might have partially executed - this is a critical state
                    Err(MigrationError::MigrationAppliedButNotRecorded {
                        name: migration_name_owned,
                        namespace: namespace_raw,
                        schema: schema_ident_raw,
                        recording_error: format!("psql exited with code {}: {}", exit_code, stderr),
                    })
                }
            }
            Err(EngineError::Io(e)) => Err(MigrationError::Database(e.into())),
        }
    }
}
