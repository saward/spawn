// This is a driver that uses a locally provided PSQL command to execute
// scripts, which enables user's scripts to take advantage of things like the
// build in PSQL helper commands.

use crate::engine::{DatabaseConfig, Engine, EngineOutputter, EngineWriter};
use anyhow::{anyhow, Result};
use std::io::{self, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

#[derive(Debug)]
pub struct PSQL {
    psql_command: Vec<String>,
    spawn_schema: String,
    spawn_database: String,
}

// Static migrations for the migration tracking table
const MIGRATION_TABLE_MIGRATIONS: &[(&str, &str)] = &[
    (
        "001_create_migration_table",
        r#"
\c {db}
CREATE SCHEMA IF NOT EXISTS {schema};
CREATE TABLE IF NOT EXISTS {schema}.migration (
    id SERIAL PRIMARY KEY,
    migration_name VARCHAR(255) NOT NULL UNIQUE,
    applied_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
"#,
    ),
    (
        "002_add_migration_index",
        r#"
\c {db}
CREATE INDEX IF NOT EXISTS idx_migration_name ON {schema}.migration (migration_name);
"#,
    ),
];

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

        Ok(Box::new(Self {
            psql_command,
            spawn_schema: config.spawn_schema.clone(),
            spawn_database: config.spawn_database.clone(),
        }))
    }
}

impl crate::engine::Engine for PSQL {
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

        Ok(Box::new(PSQLWriter { child, stdin }))
    }

    fn migration_apply(&self, migration: &str) -> Result<String> {
        // Ensure we have latest schema:
        self.update_schema()?;
        let mut writer = self.new_writer()?;

        // Write migration to writer:
        writer.write_all(migration.as_bytes())?;
        let mut outputter = writer.finalise()?;

        let output = outputter.output()?;
        // Read the Vec<u8> as utf8 or ascii:
        let output = String::from_utf8(output).unwrap_or_default();
        Ok(output)
    }
}

impl PSQL {
    pub fn update_schema(&self) -> Result<()> {
        // Check if migrations table exists
        let check_table_sql = format!(
            r#"
            \c {}
            \x
            SELECT EXISTS (
                SELECT FROM information_schema.tables
                WHERE table_schema = '{}'
                AND table_name = 'migration'
            );
            "#,
            &self.spawn_database, &self.spawn_schema
        );

        let mut writer = self.new_writer()?;
        writer.write_all(check_table_sql.as_bytes())?;
        let mut outputter = writer.finalise()?;
        let output = outputter.output()?;
        let output_str = String::from_utf8(output).unwrap_or_default();

        let table_exists = output_str.contains("exists | t");

        if !table_exists {
            // Apply all migration table migrations
            for (migration_name, migration_sql) in MIGRATION_TABLE_MIGRATIONS {
                let formatted_sql = migration_sql
                    .replace("{schema}", &self.spawn_schema)
                    .replace("{db}", &self.spawn_database);

                // Apply the migration
                let mut writer = self.new_writer()?;
                writer.write_all(formatted_sql.as_bytes())?;
                let mut outputter = writer.finalise()?;
                let _ = outputter.output()?;

                // Record the migration
                let record_sql = format!(
                    r#"\c {}
INSERT INTO {}.migration (migration_name) VALUES ('{}');"#,
                    &self.spawn_database, &self.spawn_schema, migration_name
                );
                let mut writer = self.new_writer()?;
                writer.write_all(record_sql.as_bytes())?;
                let mut outputter = writer.finalise()?;
                let _ = outputter.output()?;
            }
        } else {
            // Check which migrations have been applied
            let check_migrations_sql = format!(
                r#"\c {}

SELECT migration_name FROM {}.migration;"#,
                &self.spawn_database, &self.spawn_schema
            );

            let mut writer = self.new_writer()?;
            writer.write_all(check_migrations_sql.as_bytes())?;
            let mut outputter = writer.finalise()?;
            let output = outputter.output()?;
            let output_str = String::from_utf8(output).unwrap_or_default();

            // Apply any missing migrations
            for (migration_name, migration_sql) in MIGRATION_TABLE_MIGRATIONS {
                if !output_str.contains(migration_name) {
                    let formatted_sql = migration_sql
                        .replace("{schema}", &self.spawn_schema)
                        .replace("{db}", &self.spawn_database);

                    // Apply the migration
                    let mut writer = self.new_writer()?;
                    writer.write_all(formatted_sql.as_bytes())?;
                    let mut outputter = writer.finalise()?;
                    let _ = outputter.output()?;

                    // Record the migration
                    let record_sql = format!(
                        r#"\c {}
                        INSERT INTO {}.migration (migration_name) VALUES ('{}');"#,
                        &self.spawn_database, &self.spawn_schema, migration_name
                    );
                    let mut writer = self.new_writer()?;
                    writer.write_all(record_sql.as_bytes())?;
                    let mut outputter = writer.finalise()?;
                    let _ = outputter.output()?;
                }
            }
        }

        Ok(())
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
