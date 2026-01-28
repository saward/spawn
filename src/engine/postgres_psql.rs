// This is a driver that uses a locally provided PSQL command to execute
// scripts, which enables user's scripts to take advantage of things like the
// build in PSQL helper commands.

use crate::config::FolderPather;
use crate::engine::{
    resolve_command_spec, DatabaseConfig, Engine, EngineError, ExistingMigrationInfo,
    MigrationActivity, MigrationError, MigrationHistoryStatus, MigrationResult, MigrationStatus,
    StdoutWriter, WriterFn,
};
use crate::escape::{EscapedIdentifier, EscapedLiteral, EscapedQuery, InsecureRawSql};
use crate::sql_query;
use crate::store::pinner::latest::Latest;
use crate::store::{operator_from_includedir, Store};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use include_dir::{include_dir, Dir};
use std::collections::HashMap;
use std::collections::HashSet;
use std::io::Write;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use twox_hash::xxhash3_128;
use twox_hash::XxHash64;

/// Returns the advisory lock key used to prevent concurrent migrations.
/// This is computed as XxHash64 of "SPAWN_MIGRATION_LOCK" with seed 1234, cast to i64.
pub fn migration_lock_key() -> i64 {
    XxHash64::oneshot(1234, "SPAWN_MIGRATION_LOCK".as_bytes()) as i64
}

#[derive(Debug)]
pub struct PSQL {
    psql_command: Vec<String>,
    /// Schema name as an escaped identifier (for use as schema.table)
    spawn_schema: String,
    db_config: DatabaseConfig,
}

static PROJECT_DIR: Dir<'_> = include_dir!("./static/engine-migrations/postgres-psql");
static SPAWN_NAMESPACE: &str = "spawn";

impl PSQL {
    pub async fn new(config: &DatabaseConfig) -> Result<Box<dyn Engine>> {
        let command_spec = config
            .command
            .clone()
            .ok_or(anyhow!("Command for database config must be defined"))?;

        let psql_command = resolve_command_spec(command_spec).await?;

        let eng = Box::new(Self {
            psql_command,
            spawn_schema: config.spawn_schema.clone(),
            db_config: config.clone(),
        });

        // Ensure we have latest schema:
        eng.update_schema()
            .await
            .map_err(MigrationError::Database)?;

        Ok(eng)
    }

    fn spawn_schema_literal(&self) -> EscapedLiteral {
        EscapedLiteral::new(&self.spawn_schema)
    }

    fn spawn_schema_ident(&self) -> EscapedIdentifier {
        EscapedIdentifier::new(&self.spawn_schema)
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
        let stdout_handle = if let Some(mut stdout_dest) = stdout_writer {
            let mut stdout = child.stdout.take().expect("stdout should be piped");
            Some(tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let mut buf = Vec::new();
                let _ = stdout.read_to_end(&mut buf).await;
                let _ = stdout_dest.write_all(&buf).await;
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
        write_fn: WriterFn,
        pin_hash: Option<String>,
        namespace: &str,
        retry: bool,
    ) -> MigrationResult<String> {
        self.apply_and_record_migration_v1(
            migration_name,
            write_fn,
            pin_hash,
            EscapedLiteral::new(namespace),
            retry,
        )
        .await
    }

    async fn migration_adopt(
        &self,
        migration_name: &str,
        namespace: &str,
        description: &str,
    ) -> MigrationResult<String> {
        let namespace_lit = EscapedLiteral::new(namespace);

        // Check if migration already exists in history
        let existing_status = self
            .get_migration_status(migration_name, &namespace_lit)
            .await
            .map_err(MigrationError::Database)?;

        if let Some(info) = existing_status {
            let name = migration_name.to_string();
            let ns = namespace_lit.raw_value().to_string();

            match info.last_status {
                MigrationHistoryStatus::Success => {
                    return Err(MigrationError::AlreadyApplied {
                        name,
                        namespace: ns,
                        info,
                    });
                }
                // Allow adopting migrations that previously failed or were attempted.
                // This is one of the ways to resolve: fix manually and mark as adopted.
                MigrationHistoryStatus::Attempted | MigrationHistoryStatus::Failure => {}
            }
        }

        // Record the migration with SUCCESS status, ADOPT activity, empty checksum
        self.record_migration(
            migration_name,
            &namespace_lit,
            MigrationStatus::Success,
            MigrationActivity::Adopt,
            None, // empty checksum
            None, // no execution time
            None, // no pin_hash
            Some(description),
        )
        .await?;

        Ok(format!(
            "Migration '{}' adopted successfully",
            migration_name
        ))
    }

    async fn get_migrations_from_db(
        &self,
        namespace: Option<&str>,
    ) -> MigrationResult<Vec<crate::engine::MigrationDbInfo>> {
        use serde::Deserialize;

        // Build the query with optional namespace filter
        let namespace_lit = namespace.map(|ns| EscapedLiteral::new(ns));
        let query = sql_query!(
            r#"
            SELECT json_agg(row_to_json(t))
            FROM (
                SELECT DISTINCT ON (m.name)
                    m.name as migration_name,
                    mh.status_id_status as last_status,
                    mh.activity_id_activity as last_activity,
                    encode(mh.checksum, 'hex') as checksum
                FROM {}.migration m
                LEFT JOIN {}.migration_history mh ON m.migration_id = mh.migration_id_migration
                WHERE {} IS NULL OR m.namespace = {}
                ORDER BY m.name, mh.created_at DESC NULLS LAST
            ) t
            "#,
            self.spawn_schema_ident(),
            self.spawn_schema_ident(),
            namespace_lit,
            namespace_lit
        );

        let output = self
            .execute_sql(&query, Some("unaligned"))
            .await
            .map_err(MigrationError::Database)?;

        // Define a struct for JSON deserialization
        #[derive(Deserialize)]
        struct MigrationRow {
            migration_name: String,
            last_status: Option<String>,
            last_activity: Option<String>,
            checksum: Option<String>,
        }

        // Parse the JSON output
        let json_str = output.trim();

        // Handle case where there are no migrations (json_agg returns null)
        if json_str == "null" || json_str.is_empty() {
            return Ok(Vec::new());
        }

        let rows: Vec<MigrationRow> = serde_json::from_str(json_str).map_err(|e| {
            MigrationError::Database(anyhow::anyhow!(
                "Failed to parse JSON from database (output: '{}'): {}",
                json_str,
                e
            ))
        })?;

        // Convert to MigrationDbInfo
        let mut results: Vec<crate::engine::MigrationDbInfo> = rows
            .into_iter()
            .map(|row| {
                let status = row
                    .last_status
                    .as_deref()
                    .and_then(MigrationHistoryStatus::from_str);

                crate::engine::MigrationDbInfo {
                    migration_name: row.migration_name,
                    last_status: status,
                    last_activity: row.last_activity,
                    checksum: row.checksum,
                }
            })
            .collect();

        // Sort by migration name for consistent output
        results.sort_by(|a, b| a.migration_name.cmp(&b.migration_name));

        Ok(results)
    }
}

/// A simple AsyncWrite implementation that appends to a shared Vec<u8>
struct SharedBufWriter(Arc<Mutex<Vec<u8>>>);

impl tokio::io::AsyncWrite for SharedBufWriter {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        self.0.lock().unwrap().extend_from_slice(buf);
        std::task::Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

/// A writer that tees output to both an inner writer and a hasher for checksum calculation.
/// This allows streaming the migration SQL while computing the checksum on-the-fly.
struct TeeWriter<W: Write> {
    inner: W,
    hasher: xxhash3_128::Hasher,
}

impl<W: Write> TeeWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: xxhash3_128::Hasher::new(),
        }
    }

    fn finish(self) -> (W, u128) {
        (self.inner, self.hasher.finish_128())
    }
}

impl<W: Write> Write for TeeWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.hasher.write(buf);
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Free function to build the SQL query for recording a migration.
/// This can be used from both async and sync contexts.
fn build_record_migration_sql(
    spawn_schema: &str,
    migration_name: &str,
    namespace: &EscapedLiteral,
    status: MigrationStatus,
    activity: MigrationActivity,
    checksum: Option<&str>,
    execution_time: Option<f32>,
    pin_hash: Option<&str>,
    description: Option<&str>,
) -> EscapedQuery {
    let schema_ident = EscapedIdentifier::new(spawn_schema);
    let safe_migration_name = EscapedLiteral::new(migration_name);
    let safe_status = EscapedLiteral::new(status.as_str());
    let safe_activity = EscapedLiteral::new(activity.as_str());
    let safe_description = EscapedLiteral::new(description.unwrap_or(""));
    // If no checksum provided, use empty bytea (decode returns empty bytea for empty string)
    let checksum_expr = checksum
        .map(|c| format!("decode('{}', 'hex')", c))
        .unwrap_or_else(|| "decode('', 'hex')".to_string());
    let checksum_raw = InsecureRawSql::new(&checksum_expr);
    let safe_pin_hash = pin_hash.map(|h| EscapedLiteral::new(h));

    let duration_interval = execution_time
        .map(|d| InsecureRawSql::new(&format!("INTERVAL '{} second'", d)))
        .unwrap_or_else(|| InsecureRawSql::new("INTERVAL '0 second'"));

    sql_query!(
        r#"
BEGIN;
WITH inserted_migration AS (
    INSERT INTO {}.migration (name, namespace) VALUES ({}, {})
    ON CONFLICT (name, namespace) DO UPDATE SET name = EXCLUDED.name
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
    {},
    'unused',
    {},
    '',
    {},
    {},
    {},
    {}
FROM inserted_migration;
COMMIT;
"#,
        schema_ident,
        safe_migration_name,
        namespace,
        schema_ident,
        safe_activity,
        safe_description,
        safe_status,
        checksum_raw,
        duration_interval,
        safe_pin_hash,
    )
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
        let store = Store::new(Box::new(pinner), op.clone(), pather)
            .context("Failed to create store for update_schema")?;

        // Get list of all available migrations (sorted oldest to newest)
        let available_migrations = store
            .list_migrations()
            .await
            .context("Failed to list migrations")?;

        // Check if migration table exists to determine if this is bootstrap
        let migration_table_exists = self
            .migration_table_exists()
            .await
            .context("Failed checking if migration table exists")?;

        // Get set of already applied migrations (empty if table doesn't exist)
        let applied_migrations: HashSet<String> = if migration_table_exists {
            self.get_applied_migrations_set(&self.safe_spawn_namespace())
                .await
                .context("Failed to get applied migrations set")?
        } else {
            HashSet::new()
        };

        // Create a config to use for generating using spawn templating
        // engine.
        let mut cfg = crate::config::Config::load("spawn.toml", &op, None)
            .await
            .context("Failed to load config for postgres psql")?;
        let dbengtype = "psql".to_string();
        cfg.database = Some(dbengtype.clone());
        cfg.databases = HashMap::from([(dbengtype, self.db_config.clone())]);

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

            let migrator = crate::migrator::Migrator::new(&cfg, &migration_name, false);

            // Load and render the migration
            let variables = crate::variables::Variables::from_str(
                "json",
                &serde_json::json!({"schema": &self.spawn_schema}).to_string(),
            )?;
            let gen = migrator.generate_streaming(Some(variables)).await?;
            let mut buffer = Vec::new();
            gen.render_to_writer(&mut buffer)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            let content = String::from_utf8(buffer)?;

            // Apply the migration and record it
            // Note: even for bootstrap, the first migration creates the tables,
            // so they exist by the time we record the migration.
            let write_fn: WriterFn =
                Box::new(move |writer: &mut dyn Write| writer.write_all(content.as_bytes()));
            match self
                .apply_and_record_migration_v1(
                    migration_name,
                    write_fn,
                    None, // pin_hash not used for engine migrations
                    self.safe_spawn_namespace(),
                    false, // no retry for internal schema migrations
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
        // Use type-safe escaped types - escaping happens at construction time
        let query = sql_query!(
            r#"
            SELECT EXISTS (
                SELECT FROM information_schema.tables
                WHERE table_schema = {}
                AND table_name = {}
            );
            "#,
            self.spawn_schema_literal(),
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
            self.spawn_schema_ident(),
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
            self.spawn_schema_ident(),
            self.spawn_schema_ident(),
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

    /// Build the SQL query for recording a migration in the tracking tables.
    /// Records a migration in the tracking tables using its own psql session.
    /// Used when there is no existing writer (e.g., adopt).
    async fn record_migration(
        &self,
        migration_name: &str,
        namespace: &EscapedLiteral,
        status: MigrationStatus,
        activity: MigrationActivity,
        checksum: Option<&str>,
        execution_time: Option<f32>,
        pin_hash: Option<&str>,
        description: Option<&str>,
    ) -> MigrationResult<()> {
        let record_query = build_record_migration_sql(
            &self.spawn_schema,
            migration_name,
            namespace,
            status,
            activity,
            checksum,
            execution_time,
            pin_hash,
            description,
        );

        self.execute_with_writer(
            Box::new(move |writer| {
                writer.write_all(record_query.as_str().as_bytes())?;
                Ok(())
            }),
            None,
        )
        .await
        .map_err(|e| match e {
            EngineError::ExecutionFailed { exit_code, stderr } => {
                MigrationError::Database(anyhow!(
                    "Failed to record migration (exit {}): {}",
                    exit_code,
                    stderr
                ))
            }
            EngineError::Io(e) => MigrationError::Database(e.into()),
        })?;

        Ok(())
    }

    // This is versioned because if we change the schema significantly enough
    // later, we'll have to still write earlier migrations to the table using
    // the format of the migration table as it is at that point.
    async fn apply_and_record_migration_v1(
        &self,
        migration_name: &str,
        write_fn: WriterFn,
        pin_hash: Option<String>,
        namespace: EscapedLiteral,
        retry: bool,
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
                    if !retry {
                        return Err(MigrationError::PreviousAttemptFailed {
                            name,
                            namespace: ns,
                            status: info.last_status.clone(),
                            info,
                        });
                    }
                }
            }
        }

        let start_time = Instant::now();
        let lock_checksum = migration_lock_key();

        // Use Arc<Mutex<>> to extract checksum from the closure
        let checksum_result: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let checksum_result_clone = checksum_result.clone();

        // Session 1: Run the migration SQL only
        let migration_result = self
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

                    // Wrap the writer in a TeeWriter to compute checksum while streaming
                    let mut tee_writer = TeeWriter::new(writer);

                    // Execute the user's write function (streams migration SQL)
                    write_fn(&mut tee_writer)?;

                    // Extract the checksum
                    let (_writer, content_checksum) = tee_writer.finish();
                    let checksum_hex = format!("{:032x}", content_checksum);
                    *checksum_result_clone.lock().unwrap() = Some(checksum_hex);

                    Ok(())
                }),
                None,
            )
            .await;

        let duration = start_time.elapsed().as_secs_f32();
        let checksum_hex = checksum_result.lock().unwrap().clone();

        // Determine status based on session 1 result
        let (status, migration_error) = match &migration_result {
            Ok(()) => (MigrationStatus::Success, None),
            Err(EngineError::ExecutionFailed { exit_code, stderr }) => {
                if stderr.contains("Could not acquire advisory lock") {
                    return Err(MigrationError::AdvisoryLock(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        stderr.clone(),
                    )));
                }
                (
                    MigrationStatus::Failure,
                    Some(format!("psql exited with code {}: {}", exit_code, stderr)),
                )
            }
            Err(EngineError::Io(e)) => {
                return Err(MigrationError::Database(anyhow!(
                    "IO error running migration: {}",
                    e
                )));
            }
        };

        // Session 2: Record the outcome (success or failure)
        let record_result = self
            .record_migration(
                migration_name,
                &namespace,
                status,
                MigrationActivity::Apply,
                checksum_hex.as_deref(),
                Some(duration),
                pin_hash.as_deref(),
                None,
            )
            .await;

        // Handle recording failure
        if let Err(record_err) = record_result {
            // If migration succeeded but recording failed, that's the critical state
            if migration_error.is_none() {
                return Err(MigrationError::NotRecorded {
                    name: migration_name.to_string(),
                    migration_outcome: MigrationStatus::Success,
                    migration_error: None,
                    recording_error: format!("{}", record_err),
                });
            }
            // Both migration and recording failed
            return Err(MigrationError::NotRecorded {
                name: migration_name.to_string(),
                migration_outcome: MigrationStatus::Failure,
                migration_error: migration_error.clone(),
                recording_error: format!("{}", record_err),
            });
        }

        // If the migration itself failed (but was recorded), return that error
        if let Some(err_msg) = migration_error {
            return Err(MigrationError::Database(anyhow!(
                "Migration '{}' failed: {}",
                migration_name,
                err_msg
            )));
        }

        Ok("Migration applied successfully".to_string())
    }
}
