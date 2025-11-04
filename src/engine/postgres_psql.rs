// This is a driver that uses a locally provided PSQL command to execute
// scripts, which enables user's scripts to take advantage of things like the
// build in PSQL helper commands.

use crate::engine::{DatabaseConfig, Engine, EngineOutputter, EngineWriter};
use crate::template;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use include_dir::{include_dir, Dir, DirEntry};
use object_store::memory::InMemory;
use object_store::{ObjectStore, PutPayload};
use std::io::{self, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::Instant;
use twox_hash::xxhash3_128;

#[derive(Debug)]
pub struct PSQL {
    psql_command: Vec<String>,
    spawn_schema: String,
    spawn_database: String,
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

        Ok(Box::new(Self {
            psql_command,
            spawn_schema: config.spawn_schema.clone(),
            spawn_database: config.spawn_database.clone(),
        }))
    }
}

#[async_trait]
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

    async fn migration_apply(&self, migration: &str) -> Result<String> {
        // Ensure we have latest schema:
        self.update_schema().await?;
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
    // Helper function to recursively collect all files
    async fn collect_files(dir: &Dir<'_>, fs: &mut Box<InMemory>) -> Result<()> {
        for entry in dir.entries() {
            match entry {
                DirEntry::Dir(subdir) => Box::pin(Self::collect_files(subdir, fs)).await?,
                DirEntry::File(file) => {
                    let path = file.path().to_string_lossy().to_string();
                    let contents = file.contents().to_vec();
                    let payload: PutPayload = contents.into();
                    println!("pushing to path {:?}", path);
                    fs.put(&path.into(), payload).await?;
                }
            }
        }

        Ok(())
    }

    pub async fn update_schema(&self) -> Result<()> {
        // create a store so that we can generate our migrations using the
        // standard methods.
        let _migration_table_exists = self.migration_table_exists()?;

        // Create a memory store to use with generation:
        let mut fs = Box::new(InMemory::new());

        // Write all files from PROJECT_DIR to fs:
        PSQL::collect_files(&PROJECT_DIR, &mut fs).await?;

        template::generate_with_store(contents, variables, environment, store);

        // if migration_table_exists {
        //     // Check which migrations have been applied and apply missing ones
        //     let applied_migrations = self.get_applied_migrations()?;
        //     for (migration_name, migration_sql) in MIGRATION_TABLE_MIGRATIONS {
        //         if !applied_migrations.contains(migration_name) {
        //             self.apply_and_record_migration_v1(migration_name, migration_sql)?;
        //         }
        //     }
        // } else {
        //     // Apply all migration table migrations
        //     for (migration_name, migration_sql) in MIGRATION_TABLE_MIGRATIONS {
        //         self.apply_and_record_migration_v1(migration_name, migration_sql)?;
        //     }
        // }

        Ok(())
    }

    fn execute_sql(&self, sql: &str, format: Option<&str>) -> Result<String> {
        let mut writer = self.new_writer()?;
        writer.write_all(format!("\\c {}\n", &self.spawn_database).as_bytes())?;
        if let Some(format) = format {
            writer.write_all(format!("\\pset format {}\n", format).as_bytes())?;
        }
        writer.write_all(sql.as_bytes())?;
        let mut outputter = writer.finalise()?;
        let output = outputter.output()?;
        // Error if we can't read:
        let output = String::from_utf8(output)?;
        println!("output: {}", &output);
        Ok(output)
    }

    fn migration_table_exists(&self) -> Result<bool> {
        let check_table_sql = format!(
            r#"
            \x
            SELECT EXISTS (
                SELECT FROM information_schema.tables
                WHERE table_schema = '{}'
                AND table_name = 'migration'
            );
            "#,
            &self.spawn_schema
        );

        let output = self.execute_sql(&check_table_sql, Some("csv"))?;
        Ok(output.contains("exists,t"))
    }

    fn get_applied_migrations(&self) -> Result<String> {
        // TODO: rewrite this to return a proper HashMap of names, rather than
        // relying on crude pattern matching.
        let check_migrations_sql = format!("SELECT name FROM {}.migration;", &self.spawn_schema);

        self.execute_sql(&check_migrations_sql, Some("csv"))
    }

    // This is versioned because if we change the schema significantly enough
    // later, we'll have to still write earlier migrations to the table using
    // the format of the migration table as it is at that point.
    fn apply_and_record_migration_v1(
        &self,
        migration_name: &str,
        migration_sql: &str,
    ) -> Result<()> {
        // Apply the migration
        let formatted_sql = migration_sql.replace("{schema}", &self.spawn_schema);

        // Record duration of execute_sql:
        let start_time = Instant::now();
        self.execute_sql(&formatted_sql, None)?;
        let duration = start_time.elapsed().as_secs_f32();

        let checksum = xxhash3_128::Hasher::oneshot(&formatted_sql.as_bytes());

        // Record the migration
        let record_sql = format!(
            r#"
INSERT INTO {}.migration (name, namespace) VALUES ('{}', 'spawn');
INSERT INTO {}.migration_history (
    migration_id_migration,
    activity_id_activity,
    created_by,
    description,
    status_note,
    status_id_status,
    checksum,
    execution_time
)
SELECT
    migration_id,
    'APPLY',
    'unused',
    '',
    '',
    'SUCCESS',
    '{}',
    {}
FROM {}.migration
WHERE name = '{}' AND namespace = 'spawn';
"#,
            &self.spawn_schema,
            &migration_name,
            &self.spawn_schema,
            checksum,
            format!("INTERVAL '{} second'", duration),
            &self.spawn_schema,
            &migration_name
        );

        self.execute_sql(&record_sql, None)?;
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
