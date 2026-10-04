// This is a driver that uses a locally provided PSQL command to execute
// scripts, which enables user's scripts to take advantage of things like the
// build in PSQL helper commands.

use crate::config::FolderPather;
use crate::engine::{
    resolve_command_spec, Engine, EngineError, ExistingMigrationInfo, MigrationActivity,
    MigrationError, MigrationHistoryStatus, MigrationResult, MigrationStatus, ScriptOutput,
    ScriptSource, TargetConfig,
};
use crate::escape::{EscapedIdentifier, EscapedLiteral, EscapedQuery, InsecureRawSql};
use crate::secrets::SecretsRenderMode;
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::process::Command;
use twox_hash::XxHash64;

/// Returns the advisory lock key used to prevent concurrent migrations.
/// This is computed as XxHash64 of "SPAWN_MIGRATION_LOCK" with seed 1234, cast to i64.
pub fn migration_lock_key() -> i64 {
    XxHash64::oneshot(1234, "SPAWN_MIGRATION_LOCK".as_bytes()) as i64
}

/// Bytes of diagnostics kept for error reporting. Far more than any psql error
/// block, and bounded so a script that floods stderr cannot grow this.
const TAIL_CAP: usize = 8 * 1024;

/// Lines of that tail quoted in an error. psql error blocks are a headline plus
/// indented hints, never close to this many.
const TAIL_LINES: usize = 10;

/// Forwards to the caller's sink while keeping a bounded tail, so the engine can
/// still say why a run failed after streaming the bytes away.
struct TailCapture<'a> {
    inner: &'a mut (dyn tokio::io::AsyncWrite + Send + Unpin),
    tail: Vec<u8>,
    dropped: bool,
}

impl<'a> TailCapture<'a> {
    fn new(inner: &'a mut (dyn tokio::io::AsyncWrite + Send + Unpin)) -> Self {
        Self {
            inner,
            tail: Vec::new(),
            dropped: false,
        }
    }
}

impl tokio::io::AsyncWrite for TailCapture<'_> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        match std::pin::Pin::new(&mut *this.inner).poll_write(cx, buf) {
            std::task::Poll::Ready(Ok(n)) => {
                this.tail.extend_from_slice(&buf[..n]);
                if this.tail.len() > TAIL_CAP {
                    let excess = this.tail.len() - TAIL_CAP;
                    this.tail.drain(..excess);
                    this.dropped = true;
                }
                std::task::Poll::Ready(Ok(n))
            }
            other => other,
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut *self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut *self.get_mut().inner).poll_shutdown(cx)
    }
}

/// The last whole lines of a captured tail, for quoting in an error.
///
/// Crops only at line boundaries: a byte cap can land inside a line, so the
/// leading partial one is discarded rather than shown.
fn tail_summary(tail: &[u8], dropped: bool) -> Option<String> {
    let text = String::from_utf8_lossy(tail);

    let mut body: &str = &text;
    if dropped {
        body = match body.find('\n') {
            Some(i) => &body[i + 1..],
            None => "",
        };
    }

    let body = body.trim_end();
    if body.is_empty() {
        return None;
    }

    let lines: Vec<&str> = body.lines().collect();
    let start = lines.len().saturating_sub(TAIL_LINES);
    let joined = lines[start..].join("\n");

    Some(if dropped || start > 0 {
        format!("…\n{}", joined)
    } else {
        joined
    })
}

#[derive(Debug)]
pub struct PSQL {
    psql_command: Vec<String>,
    target_config: TargetConfig,
    // Set once update_schema confirms the tracking tables exist.
    schema_ready: AtomicBool,
}

static PROJECT_DIR: Dir<'_> = include_dir!("./static/engine-migrations/postgres-psql");
static SPAWN_NAMESPACE: &str = "spawn";

impl PSQL {
    pub async fn new(config: &TargetConfig) -> Result<Box<dyn Engine>> {
        let command_spec = config
            .command
            .clone()
            .ok_or(anyhow!("Command for target config must be defined"))?;

        let psql_command = resolve_command_spec(command_spec).await?;

        let eng = Box::new(Self {
            psql_command,
            target_config: config.clone(),
            schema_ready: AtomicBool::new(false),
        });

        // Ensure we have latest schema:
        eng.update_schema()
            .await
            .map_err(MigrationError::Database)?;

        Ok(eng)
    }

    fn spawn_schema_literal(&self) -> EscapedLiteral {
        EscapedLiteral::new(&self.target_config.spawn_schema)
    }

    fn spawn_schema_ident(&self) -> EscapedIdentifier {
        EscapedIdentifier::new(&self.target_config.spawn_schema)
    }

    fn safe_spawn_namespace(&self) -> EscapedLiteral {
        EscapedLiteral::new(SPAWN_NAMESPACE)
    }

    /// Returns a psql `\c` command to switch to the given database, or
    /// an empty string if `database` is None.
    fn db_connect_command(database: Option<&str>) -> InsecureRawSql {
        if let Some(db) = database {
            InsecureRawSql::new(&format!("\\c {}\n", EscapedIdentifier::new(db)))
        } else {
            InsecureRawSql::new("")
        }
    }

    /// Returns a psql `\c` command to switch to the spawn_database, if configured.
    /// Used to ensure internal queries and schema migrations target the correct database.
    fn spawn_db_connect_command(&self) -> InsecureRawSql {
        Self::db_connect_command(self.target_config.spawn_database.as_deref())
    }

    fn build_record_migration_sql(
        &self,
        migration_name: &str,
        namespace: &EscapedLiteral,
        status: MigrationStatus,
        activity: MigrationActivity,
        checksum: Option<&str>,
        execution_time: Option<f32>,
        pin_hash: Option<&str>,
        description: Option<&str>,
    ) -> EscapedQuery {
        let safe_migration_name = EscapedLiteral::new(migration_name);
        let safe_status = EscapedLiteral::new(status.as_str());
        let safe_activity = EscapedLiteral::new(activity.as_str());
        let safe_description = EscapedLiteral::new(description.unwrap_or(""));
        // If no checksum provided, use empty bytea (decode returns empty bytea for empty string).
        // checksum is caller-supplied, so it's escaped via EscapedLiteral rather than
        // interpolated directly — decode() itself rejects anything that isn't valid hex.
        let checksum_expr = format!(
            "decode({}, 'hex')",
            EscapedLiteral::new(checksum.unwrap_or(""))
        );
        let checksum_raw = InsecureRawSql::new(&checksum_expr);
        let safe_pin_hash = pin_hash.map(|h| EscapedLiteral::new(h));

        let duration_interval = execution_time
            .map(|d| InsecureRawSql::new(&format!("INTERVAL '{} second'", d)))
            .unwrap_or_else(|| InsecureRawSql::new("INTERVAL '0 second'"));

        let qry = sql_query!(
            r#"
    {}
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
            self.spawn_db_connect_command(),
            self.spawn_schema_ident(),
            safe_migration_name,
            namespace,
            self.spawn_schema_ident(),
            safe_activity,
            safe_description,
            safe_status,
            checksum_raw,
            duration_interval,
            safe_pin_hash,
        );
        qry
    }
}

#[async_trait]
impl Engine for PSQL {
    async fn run_script(
        &self,
        script: ScriptSource,
        out: ScriptOutput<'_>,
    ) -> Result<(), EngineError> {
        let ScriptOutput {
            results,
            diagnostics,
        } = out;
        let mut diagnostics = TailCapture::new(diagnostics);

        let outcome = self
            .execute_with_writer(script, Some(results), &mut diagnostics)
            .await;

        match outcome {
            Ok(()) => Ok(()),
            // Exit 3 is a statement failing under ON_ERROR_STOP, which is on by
            // default but a script may turn off. Either way its diagnostics are
            // already written, so this is a normal outcome.
            Err(EngineError::ExecutionFailed { exit_code: 3 }) => Ok(()),
            // Exit 1 is psql itself failing, 2 a bad or lost connection. The
            // reason went to the caller's sink, so quote the tail of it here:
            // a caller streaming to disk cannot cheaply go back and look.
            Err(EngineError::ExecutionFailed { exit_code }) => {
                let message = match tail_summary(&diagnostics.tail, diagnostics.dropped) {
                    Some(detail) => format!("psql exited with code {}: {}", exit_code, detail),
                    None => format!("psql exited with code {}", exit_code),
                };
                Err(EngineError::Unavailable { message })
            }
            Err(e) => Err(e),
        }
    }

    async fn migration_apply(
        &self,
        migration_name: &str,
        script: ScriptSource,
        checksum: String,
        pin_hash: Option<String>,
        namespace: &str,
        retry: bool,
    ) -> MigrationResult<String> {
        self.apply_and_record_migration_v1(
            migration_name,
            script,
            checksum,
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
            .execute_sql(
                &query,
                Some("unaligned"),
                self.target_config.spawn_database.as_deref(),
            )
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

impl PSQL {
    /// Runs `script` through psql, streaming stdout to `results` (when given)
    /// and stderr to `diagnostics`.
    ///
    /// Both pipes are drained concurrently with the write to stdin: psql blocks
    /// once either fills, so draining them only afterwards would deadlock.
    async fn execute_with_writer(
        &self,
        script: ScriptSource,
        results: Option<&mut (dyn tokio::io::AsyncWrite + Send + Unpin)>,
        diagnostics: &mut (dyn tokio::io::AsyncWrite + Send + Unpin),
    ) -> Result<(), EngineError> {
        let (reader, mut writer) = std::io::pipe()?;

        let stdout_config = if results.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        };
        let mut child = Command::new(&self.psql_command[0])
            .args(&self.psql_command[1..])
            .stdin(Stdio::from(reader))
            .stdout(stdout_config)
            .stderr(Stdio::piped())
            .spawn()
            .map_err(EngineError::Io)?;

        let mut child_stdout = child.stdout.take();
        let mut child_stderr = child.stderr.take().expect("stderr should be piped");

        // Blocking thread: the render blocks, resolving secrets and loading
        // components via `block_in_place`.
        let writer_handle = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            // QUIET must come first to suppress output from the other settings.
            // All three are defaults a script is free to override.
            writer.write_all(b"\\set QUIET on\n")?;
            writer.write_all(b"\\pset pager off\n")?;
            writer.write_all(b"\\set ON_ERROR_STOP on\n")?;

            script(&mut writer)?;

            // Writer dropped here -> EOF to psql
            Ok(())
        });

        // A sink that fails must not stop us reading its pipe: psql blocks once
        // the pipe fills, which stalls the other two futures and hangs the
        // join. Drain to discard instead, and report the original error.
        let copy_results = async {
            match (child_stdout.as_mut(), results) {
                (Some(src), Some(dst)) => {
                    let result = tokio::io::copy(src, dst).await;
                    if result.is_err() {
                        let mut discard = tokio::io::sink();
                        let _ = tokio::io::copy(src, &mut discard).await;
                    }
                    result.map(|_| ())
                }
                _ => Ok(()),
            }
        };
        let copy_diagnostics = async {
            let result = tokio::io::copy(&mut child_stderr, diagnostics).await;
            if result.is_err() {
                let mut discard = tokio::io::sink();
                let _ = tokio::io::copy(&mut child_stderr, &mut discard).await;
            }
            result.map(|_| ())
        };

        // join!, not spawn: the sinks are borrowed, so they cannot be moved into
        // a 'static task.
        let (writer_result, results_result, diagnostics_result) =
            tokio::join!(writer_handle, copy_results, copy_diagnostics);

        let status = child.wait().await?;

        // Takes precedence over psql's own status: an aborted render can leave
        // psql exiting 0 (e.g. a quiet rollback on early EOF).
        writer_result
            .map_err(|e| EngineError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))
            .and_then(|r| r.map_err(EngineError::Io))?;
        results_result?;
        diagnostics_result?;

        if !status.success() {
            return Err(EngineError::ExecutionFailed {
                exit_code: status.code().unwrap_or(-1),
            });
        }

        Ok(())
    }

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
        let mut cfg = crate::config::Config::load(crate::config::DEFAULT_CONFIG_FILE, &op, None)
            .await
            .context("Failed to load config for postgres psql")?;
        let dbengtype = "psql".to_string();
        cfg.target = Some(dbengtype.clone());
        cfg.targets = HashMap::from([(dbengtype, self.target_config.clone())]);

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
                &serde_json::json!({"schema": &self.target_config.spawn_schema}).to_string(),
            )?;
            let gen = migrator
                .generate_streaming(Some(variables), SecretsRenderMode::Revealed)
                .await?;
            let checksum = gen.raw_checksum();
            let mut buffer = Vec::new();
            gen.render_to_writer(&mut buffer)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            let content = String::from_utf8(buffer)?;

            // Apply the migration and record it
            // Note: even for bootstrap, the first migration creates the tables,
            // so they exist by the time we record the migration.
            // Prefix with \c to spawn_database if configured, so the internal
            // schema is created in the correct database.
            let db_connect = self.spawn_db_connect_command();
            let script: ScriptSource = Box::new(move |writer: &mut dyn Write| {
                writer.write_all(db_connect.as_str().as_bytes())?;
                writer.write_all(content.as_bytes())
            });
            match self
                .apply_and_record_migration_v1(
                    migration_name,
                    script,
                    checksum,
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

        self.schema_ready.store(true, Ordering::Relaxed);

        Ok(())
    }

    /// Execute SQL and return stdout as a String.
    /// Used for internal queries where we need to parse results.
    /// If `database` is Some, a `\c` command is prepended to switch databases first.
    async fn execute_sql(
        &self,
        query: &EscapedQuery,
        format: Option<&str>,
        database: Option<&str>,
    ) -> Result<String> {
        let query_str = query.as_str().to_string();
        let format_owned = format.map(|s| s.to_string());
        let db_connect = Self::db_connect_command(database);

        let mut stdout_buf: Vec<u8> = Vec::new();
        let mut stderr_buf: Vec<u8> = Vec::new();

        let outcome = self
            .execute_with_writer(
                Box::new(move |writer| {
                    // Switch database if requested
                    writer.write_all(db_connect.as_str().as_bytes())?;
                    // Format settings if requested (QUIET is already set globally)
                    if let Some(fmt) = format_owned {
                        writer.write_all(b"\\pset tuples_only on\n")?;
                        writer.write_all(format!("\\pset format {}\n", fmt).as_bytes())?;
                    }
                    writer.write_all(query_str.as_bytes())?;
                    Ok(())
                }),
                Some(&mut stdout_buf as &mut (dyn tokio::io::AsyncWrite + Send + Unpin)),
                &mut stderr_buf,
            )
            .await;

        if let Err(e) = outcome {
            return Err(anyhow!(
                "SQL execution failed: {}: {}",
                e,
                String::from_utf8_lossy(&stderr_buf).trim()
            ));
        }

        Ok(String::from_utf8_lossy(&stdout_buf).to_string())
    }

    async fn migration_table_exists(&self) -> Result<bool> {
        self.spawn_table_exists("migration").await
    }

    async fn migration_history_table_exists(&self) -> Result<bool> {
        self.spawn_table_exists("migration_history").await
    }

    async fn spawn_table_exists(&self, table_name: &str) -> Result<bool> {
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

        let output = self
            .execute_sql(
                &query,
                Some("csv"),
                self.target_config.spawn_database.as_deref(),
            )
            .await?;
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

        let output = self
            .execute_sql(
                &query,
                Some("csv"),
                self.target_config.spawn_database.as_deref(),
            )
            .await?;
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

        let output = self
            .execute_sql(
                &query,
                Some("csv"),
                self.target_config.spawn_database.as_deref(),
            )
            .await?;

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
        let record_query = self.build_record_migration_sql(
            migration_name,
            namespace,
            status,
            activity,
            checksum,
            execution_time,
            pin_hash,
            description,
        );

        let mut diagnostics: Vec<u8> = Vec::new();
        let outcome = self
            .execute_with_writer(
                Box::new(move |writer| {
                    writer.write_all(record_query.as_str().as_bytes())?;
                    Ok(())
                }),
                None,
                &mut diagnostics,
            )
            .await;

        if let Err(e) = outcome {
            return Err(MigrationError::Database(anyhow!(
                "Failed to record migration ({}): {}",
                e,
                String::from_utf8_lossy(&diagnostics).trim()
            )));
        }

        Ok(())
    }

    // This is versioned because if we change the schema significantly enough
    // later, we'll have to still write earlier migrations to the table using
    // the format of the migration table as it is at that point.
    async fn apply_and_record_migration_v1(
        &self,
        migration_name: &str,
        script: ScriptSource,
        checksum: String,
        pin_hash: Option<String>,
        namespace: EscapedLiteral,
        retry: bool,
    ) -> MigrationResult<String> {
        // If schema is ready, then history table must exist.
        let history_table_exists = if self.schema_ready.load(Ordering::Relaxed) {
            true
        } else {
            self.migration_history_table_exists()
                .await
                .map_err(MigrationError::Database)?
        };
        let existing_status = if history_table_exists {
            self.get_migration_status(migration_name, &namespace)
                .await
                .map_err(MigrationError::Database)?
        } else {
            None
        };

        if let Some(info) = existing_status {
            if !retry {
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
        }

        let start_time = Instant::now();
        let lock_checksum = migration_lock_key();

        // Session 1: Run the migration SQL only.
        //
        // Bounded: ordinary DDL emits a NOTICE per statement ("will create
        // implicit index", "does not exist, skipping"), so a large migration
        // produces megabytes of them on a perfectly successful run. Only the
        // tail is kept, which is where a failure lands.
        let mut discard = tokio::io::sink();
        let mut diagnostics = TailCapture::new(&mut discard);
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

                    // Stream the migration SQL straight through — no buffering,
                    // no tee'd hashing. The checksum is a hash of the migration's
                    // raw template source (see StreamingGeneration::raw_checksum),
                    // computed by the caller before this closure ever runs, so it
                    // can never contain a resolved secret value.
                    script(writer)?;

                    Ok(())
                }),
                None,
                &mut diagnostics,
            )
            .await;

        let duration = start_time.elapsed().as_secs_f32();

        // Determine status based on session 1 result.
        let (status, migration_error): (MigrationStatus, Option<anyhow::Error>) =
            match migration_result {
                Ok(()) => (MigrationStatus::Success, None),
                Err(EngineError::ExecutionFailed { exit_code }) => {
                    // Safe against the tail evicting it: spawn sets
                    // ON_ERROR_STOP before the lock statement, so a failed lock
                    // exits psql immediately and nothing follows it.
                    let stderr =
                        tail_summary(&diagnostics.tail, diagnostics.dropped).unwrap_or_default();
                    if stderr.contains("Could not acquire advisory lock") {
                        return Err(MigrationError::AdvisoryLock(std::io::Error::other(stderr)));
                    }
                    (
                        MigrationStatus::Failure,
                        Some(anyhow!("psql exited with code {}: {}", exit_code, stderr)),
                    )
                }
                Err(EngineError::Io(e)) => {
                    // Writer failed (e.g. an unresolved secret), not psql itself.
                    // Still record a Failure row — SQL may already have run.
                    (
                        MigrationStatus::Failure,
                        Some(
                            anyhow::Error::from(e).context("failed while streaming migration SQL"),
                        ),
                    )
                }
                // Only `run_script` produces this; `execute_with_writer` never does.
                Err(EngineError::Unavailable { message }) => {
                    (MigrationStatus::Failure, Some(anyhow!("{}", message)))
                }
            };

        // NotRecorded holds strings; `{:#}` flattens the chain onto one line.
        let migration_error_text = migration_error.as_ref().map(|e| format!("{:#}", e));

        // Session 2: Record the outcome (success or failure)
        let record_result = self
            .record_migration(
                migration_name,
                &namespace,
                status,
                MigrationActivity::Apply,
                Some(checksum.as_str()),
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
                    recording_error: format!("{:#}", anyhow::Error::new(record_err)),
                });
            }
            // Both migration and recording failed
            return Err(MigrationError::NotRecorded {
                name: migration_name.to_string(),
                migration_outcome: MigrationStatus::Failure,
                migration_error: migration_error_text,
                recording_error: format!("{:#}", anyhow::Error::new(record_err)),
            });
        }

        // If the migration itself failed (but was recorded), return that error.
        // A context layer, not an interpolated string, so the chain survives.
        if let Some(err) = migration_error {
            return Err(MigrationError::Database(
                err.context(format!("Migration '{}' failed", migration_name)),
            ));
        }

        Ok("Migration applied successfully".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The byte cap bounds memory, which no error message can reveal: whatever
    // was buffered, `tail_summary` only ever quotes its last few lines. So the
    // cap has to be checked on the buffer itself.
    #[tokio::test]
    async fn tail_capture_bounds_what_it_keeps_and_keeps_the_end() {
        use tokio::io::AsyncWriteExt;

        let mut discard = tokio::io::sink();
        let mut capture = TailCapture::new(&mut discard);

        for i in 0..2000 {
            capture
                .write_all(format!("NOTICE:  line {}\n", i).as_bytes())
                .await
                .unwrap();
        }

        assert!(
            capture.tail.len() <= TAIL_CAP,
            "kept {} bytes, cap is {}",
            capture.tail.len(),
            TAIL_CAP
        );
        assert!(
            capture.dropped,
            "should have recorded that it discarded data"
        );

        let kept = String::from_utf8_lossy(&capture.tail);
        assert!(kept.ends_with("NOTICE:  line 1999\n"), "got: {:?}", kept);
        assert!(!kept.contains("line 0\n"), "the start should be gone");
    }

    #[test]
    fn tail_summary_returns_a_short_block_whole() {
        let stderr = "psql: error: connection to server failed: Connection refused\n\
                      \tIs the server running on that host?\n";
        assert_eq!(
            tail_summary(stderr.as_bytes(), false).unwrap(),
            "psql: error: connection to server failed: Connection refused\n\
             \tIs the server running on that host?"
        );
    }

    #[test]
    fn tail_summary_is_none_when_nothing_was_written() {
        assert_eq!(tail_summary(b"", false), None);
        assert_eq!(tail_summary(b"  \n\n", false), None);
    }

    #[test]
    fn tail_summary_keeps_the_last_lines_not_the_first() {
        // The failure is always last, so that is the end worth quoting.
        let mut stderr = String::new();
        for i in 0..100 {
            stderr.push_str(&format!("NOTICE:  noise {}\n", i));
        }
        stderr.push_str("psql: error: connection to server was lost\n");

        let summary = tail_summary(stderr.as_bytes(), false).unwrap();
        assert!(summary.ends_with("psql: error: connection to server was lost"));
        assert_eq!(summary.lines().count(), TAIL_LINES + 1); // + the "…" marker
        assert!(summary.starts_with("…\n"));
        assert!(!summary.contains("noise 0\n"));
    }

    // A byte cap cannot know where lines are, so the first line of a capped
    // tail is usually a fragment. It must be dropped, not shown.
    #[test]
    fn tail_summary_drops_the_partial_line_left_by_a_byte_cap() {
        let summary = tail_summary(b"ection refused\nFATAL:  the real error\n", true).unwrap();
        assert_eq!(summary, "…\nFATAL:  the real error");
        assert!(!summary.contains("ection refused"));
    }

    #[test]
    fn tail_summary_is_none_when_the_cap_left_only_a_fragment() {
        assert_eq!(tail_summary(b"a dangling fragment", true), None);
    }
}
