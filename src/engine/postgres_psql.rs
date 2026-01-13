// This is a driver that uses a locally provided PSQL command to execute
// scripts, which enables user's scripts to take advantage of things like the
// build in PSQL helper commands.

use crate::config::FolderPather;
use crate::engine::{DatabaseConfig, Engine, EngineOutputter, EngineWriter};
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

#[derive(Debug)]
pub struct PSQL {
    psql_command: Vec<String>,
    /// Schema name as an escaped identifier (for use as schema.table)
    spawn_schema_ident: EscapedIdentifier,
    /// Schema name as an escaped literal (for use in WHERE clauses)
    spawn_schema_literal: EscapedLiteral,
}

static PROJECT_DIR: Dir<'_> = include_dir!("./static/engine-migrations/postgres-psql");

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
            .ok_or(anyhow!("Command command must be defined"))?;

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
        EscapedLiteral::new("spawn")
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
    ) -> Result<String> {
        // Ensure we have latest schema:
        self.update_schema().await?;
        return self.apply_and_record_migration_v1(
            migration_name,
            migration,
            pin_hash,
            EscapedLiteral::new(namespace),
        );
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
            self.get_applied_migrations_set()?
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
            self.apply_and_record_migration_v1(
                migration_name,
                &rendered_sql,
                None, // pin_hash not used for engine migrations
                self.safe_spawn_namespace(),
            )?;
        }

        Ok(())
    }

    fn execute_sql(&self, query: &EscapedQuery, format: Option<&str>) -> Result<String> {
        let mut writer = self.new_writer()?;
        // Assumes psql_command already connects to the correct database
        if let Some(format) = format {
            writer.write_all(format!("\\pset format {}\n", format).as_bytes())?;
        }
        writer.write_all(query.as_str().as_bytes())?;
        let mut outputter = writer.finalise()?;
        let output = outputter.output()?;
        // Error if we can't read:
        let output = String::from_utf8(output)?;
        println!("output: {}", &output);
        Ok(output)
    }

    fn migration_table_exists(&self) -> Result<bool> {
        let query = sql_query!(
            r#"
            \x
            SELECT EXISTS (
                SELECT FROM information_schema.tables
                WHERE table_schema = {}
                AND table_name = 'migration'
            );
            "#,
            self.spawn_schema_literal
        );

        let output = self.execute_sql(&query, Some("csv"))?;
        Ok(output.contains("exists,t"))
    }

    fn get_applied_migrations(&self) -> Result<String> {
        // TODO: rewrite this to return a proper HashMap of names, rather than
        // relying on crude pattern matching.
        let query = sql_query!(
            "SELECT name FROM {}.migration WHERE namespace = 'spawn';",
            self.spawn_schema_ident
        );

        self.execute_sql(&query, Some("csv"))
    }

    fn get_applied_migrations_set(&self) -> Result<HashSet<String>> {
        let output = self.get_applied_migrations()?;
        let mut migrations = HashSet::new();

        // Parse CSV output - skip header line and extract migration names
        for line in output.lines().skip(1) {
            let name = line.trim();
            if !name.is_empty() {
                migrations.insert(name.to_string());
            }
        }

        Ok(migrations)
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
    ) -> Result<String> {
        Check if migration is already applied and don't apply again
        // Record duration of execute_sql:
        let start_time = Instant::now();
        // We need to trust that the migration_sql is already safely escaped
        // for now:
        let migration_sql = sql_query!("{}", InsecureRawSql::new(migration_sql));
        self.execute_sql(&migration_sql, None)?;
        let duration = start_time.elapsed().as_secs_f32();

        let checksum = xxhash3_128::Hasher::oneshot(migration_sql.as_str().as_bytes());

        // Use type-safe escaped literals for dynamic values
        let safe_migration_name = EscapedLiteral::new(migration_name);
        let safe_checksum = EscapedLiteral::new(&format!("{:032x}", checksum));
        let safe_pin_hash = pin_hash.map(|hash| EscapedLiteral::new(&hash));

        // Record the migration (schema is pre-escaped in struct)
        let duration_interval = InsecureRawSql::new(&format!("INTERVAL '{} second'", duration));
        let record_query = sql_query!(
            r#"
BEGIN;
INSERT INTO {}.migration (name, namespace) VALUES ({}, 'spawn');
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
FROM {}.migration
WHERE name = {} AND namespace = {};
COMMIT;
"#,
            self.spawn_schema_ident,
            safe_migration_name,
            self.spawn_schema_ident,
            safe_checksum,
            duration_interval,
            safe_pin_hash,
            self.spawn_schema_ident,
            safe_migration_name,
            namespace,
        );

        self.execute_sql(&record_query, None)
    }
}

impl crate::engine::EngineOutputter for PSQLOutput {
    fn output(&mut self) -> io::Result<Vec<u8>> {
        // Collect all output
        let mut output = Vec::new();
        if let Some(mut out) = self.child.stdout.take() {
            out.read_to_end(&mut output)?;
        }

        // Reap the child to avoid a zombie process
        let _ = self.child.wait()?;
        Ok(output)
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
