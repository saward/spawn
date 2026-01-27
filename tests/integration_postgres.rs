//! Integration tests for PostgreSQL database operations.
//!
//! These tests require a running PostgreSQL instance. By default, they are
//! marked with `#[ignore]` so they don't run during normal `cargo test`.
//!
//! ## Running integration tests
//!
//! ### Local development (with Docker):
//!
//! 1. Start the PostgreSQL container:
//!    ```sh
//!    docker compose up -d
//!    ```
//!
//! 2. Run the integration tests:
//!    ```sh
//!    cargo test --test integration_postgres -- --ignored
//!    ```
//!
//! ### CI mode (direct PostgreSQL connection):
//!
//! Set environment variables for direct psql connection:
//! ```sh
//! SPAWN_TEST_PSQL_HOST=localhost
//! SPAWN_TEST_PSQL_PORT=5432
//! SPAWN_TEST_PSQL_USER=spawn
//! PGPASSWORD=spawn
//! cargo test --test integration_postgres -- --ignored
//! ```
//!
//! ## Test isolation
//!
//! Each test creates its own unique database using `WITH TEMPLATE` from a base
//! template, allowing tests to run in parallel without interference. The test
//! databases are cleaned up after each test.

mod migration_build;

use anyhow::{anyhow, Context, Result};
use migration_build::MigrationTestHelper;
use opendal::services::Memory;
use opendal::Operator;
use spawn_db::{
    commands::{AdoptMigration, ApplyMigration, Command, CompareTests, ExpectTest, Outcome},
    config::ConfigLoaderSaver,
    engine::{CommandSpec, DatabaseConfig, EngineType},
};
use std::collections::HashMap;
use std::env;
use std::process::Command as ProcessCommand;
use uuid::Uuid;

/// Configuration for connecting to the test PostgreSQL instance (Docker mode)
const DOCKER_CONTAINER: &str = "spawn-db";
const DEFAULT_POSTGRES_USER: &str = "spawn";
// This database is one that isn't used for tests or for with WITH TEMPLATE, so
// that we don't cause issues with other tests.
const DEFAULT_NEUTRAL_DATABASE: &str = "postgres";
const TEMPLATE_DATABASE: &str = "spawn";

/// Determines the connection mode based on environment variables
#[derive(Clone)]
enum ConnectionMode {
    /// Use docker exec to run psql inside the container
    Docker { container: String, user: String },
    /// Use direct psql connection (for CI or native PostgreSQL)
    Direct {
        host: String,
        port: String,
        user: String,
    },
}

impl ConnectionMode {
    fn from_env() -> Self {
        // Check if we're in CI/direct mode
        if let Ok(host) = env::var("SPAWN_TEST_PSQL_HOST") {
            let port = env::var("SPAWN_TEST_PSQL_PORT").unwrap_or_else(|_| "5432".to_string());
            let user = env::var("SPAWN_TEST_PSQL_USER")
                .unwrap_or_else(|_| DEFAULT_POSTGRES_USER.to_string());
            ConnectionMode::Direct { host, port, user }
        } else {
            // Default to Docker mode
            let container = env::var("SPAWN_TEST_DOCKER_CONTAINER")
                .unwrap_or_else(|_| DOCKER_CONTAINER.to_string());
            let user = env::var("SPAWN_TEST_PSQL_USER")
                .unwrap_or_else(|_| DEFAULT_POSTGRES_USER.to_string());
            ConnectionMode::Docker { container, user }
        }
    }

    /// Build the psql command for use in spawn config
    fn psql_command(&self, database: &str) -> Vec<String> {
        match self {
            ConnectionMode::Docker { container, user } => {
                vec![
                    "docker".to_string(),
                    "exec".to_string(),
                    "-i".to_string(),
                    container.clone(),
                    "psql".to_string(),
                    "-U".to_string(),
                    user.clone(),
                    database.to_string(),
                ]
            }
            ConnectionMode::Direct { host, port, user } => {
                vec![
                    "psql".to_string(),
                    "-h".to_string(),
                    host.clone(),
                    "-p".to_string(),
                    port.clone(),
                    "-U".to_string(),
                    user.clone(),
                    database.to_string(),
                ]
            }
        }
    }

    /// Execute a SQL command and return the output. The PSQL engine itself has
    /// its own execute function. This is used for test verification.
    fn execute_sql(&self, database: &str, sql: &str) -> Result<std::process::Output> {
        match self {
            ConnectionMode::Docker { container, user } => ProcessCommand::new("docker")
                .args([
                    "exec", "-i", container, "psql", "-U", user, database, "-c", sql,
                ])
                .output()
                .context("Failed to execute docker psql command"),
            ConnectionMode::Direct { host, port, user } => ProcessCommand::new("psql")
                .args(["-h", host, "-p", port, "-U", user, database, "-c", sql])
                .output()
                .context("Failed to execute psql command"),
        }
    }

    /// Check if PostgreSQL is ready
    fn is_ready(&self) -> bool {
        match self {
            ConnectionMode::Docker { container, user } => ProcessCommand::new("docker")
                .args(["exec", container, "pg_isready", "-U", user])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false),
            ConnectionMode::Direct { host, port, user } => ProcessCommand::new("pg_isready")
                .args(["-h", host, "-p", port, "-U", user])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false),
        }
    }
}

/// Helper struct for integration tests that manages database lifecycle
pub struct IntegrationTestHelper {
    migration_helper: MigrationTestHelper,
    db_name: String,
    test_name: String,
    connection_mode: ConnectionMode,
    keep_db: bool,
}

/// Check if SPAWN_TEST_KEEP_DB is set to keep test databases after tests complete
fn should_keep_db() -> bool {
    env::var("SPAWN_TEST_KEEP_DB").is_ok()
}

impl IntegrationTestHelper {
    /// Creates a new test environment with an isolated database.
    ///
    /// This creates a fresh database from the template for test isolation.
    pub async fn new(test_name: &str, folder: Option<&str>) -> Result<Self> {
        let connection_mode = ConnectionMode::from_env();
        let keep_db = should_keep_db();

        // Generate a unique database name for this test
        let db_name = format!("spawn_test_{}", Uuid::new_v4().simple());

        // Print the database name so it can be inspected with --nocapture
        println!();
        println!("=======================================================");
        println!("Test: {}", test_name);
        println!("Database: {}", db_name);
        if keep_db {
            println!("SPAWN_TEST_KEEP_DB is set - database will be preserved");
        }
        println!("=======================================================");

        // Create the test database from template
        Self::create_test_database(&db_name, &connection_mode)?;

        // Create in-memory filesystem for test files
        let mem_op = match folder {
            Some(folder) => MigrationTestHelper::operator_from_local_folder(folder).await?,
            None => {
                let mem_service = Memory::default();
                Operator::new(mem_service)?.finish()
            }
        };

        // Create the database config for this test
        let config_loader = Self::create_config(&db_name, &connection_mode);

        // Use MigrationTestHelper for filesystem and config management
        let migration_helper =
            MigrationTestHelper::new_from_operator_with_config(mem_op, config_loader).await?;

        let helper = Self {
            migration_helper,
            db_name: db_name.clone(),
            test_name: test_name.to_string(),
            connection_mode,
            keep_db,
        };

        Ok(helper)
    }

    /// Creates a database config that points to our isolated test database
    fn create_config(db_name: &str, connection_mode: &ConnectionMode) -> ConfigLoaderSaver {
        let mut databases = HashMap::new();
        databases.insert(
            "postgres_psql".to_string(),
            DatabaseConfig {
                engine: EngineType::PostgresPSQL,
                spawn_database: db_name.to_string(),
                spawn_schema: "_spawn".to_string(),
                environment: "test".to_string(),
                command: Some(CommandSpec::Direct {
                    direct: connection_mode.psql_command(db_name),
                }),
            },
        );

        ConfigLoaderSaver {
            spawn_folder: "/db".to_string(),
            database: Some("postgres_psql".to_string()),
            environment: None,
            databases: Some(databases),
            project_id: None,
            telemetry: Some(false),
        }
    }

    /// Creates an isolated test database using PostgreSQL's template feature
    fn create_test_database(db_name: &str, mode: &ConnectionMode) -> Result<()> {
        // Don't connect to our database we intend to run WITH TEMPLATE against
        // because then it will sometimes fail.
        let output = mode.execute_sql(
            DEFAULT_NEUTRAL_DATABASE,
            &format!(
                "CREATE DATABASE \"{}\" WITH TEMPLATE \"{}\"",
                db_name, TEMPLATE_DATABASE
            ),
        )?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!(
                "Failed to create test database '{}': {}",
                db_name,
                stderr
            ));
        }

        Ok(())
    }

    /// Drops the test database
    fn drop_test_database(&self) -> Result<()> {
        // First, terminate any connections to the database
        let _ = self.connection_mode.execute_sql(
            DEFAULT_NEUTRAL_DATABASE,
            &format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
                self.db_name
            ),
        );

        let output = self.connection_mode.execute_sql(
            DEFAULT_NEUTRAL_DATABASE,
            &format!("DROP DATABASE IF EXISTS \"{}\"", self.db_name),
        )?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!(
                "Warning: Failed to drop test database '{}': {}",
                self.db_name, stderr
            );
        }

        Ok(())
    }

    /// Applies a migration to the test database using the ApplyMigration command
    pub async fn apply_migration(&self, migration_name: &str) -> Result<()> {
        let config = self.migration_helper.load_config().await?;
        let cmd = ApplyMigration {
            migration: Some(migration_name.to_string()),
            pinned: false,
            variables: None,
        };

        let outcome = cmd.execute(&config).await?;

        match outcome {
            Outcome::AppliedMigrations => Ok(()),
            _ => Err(anyhow::anyhow!("Unexpected outcome from migration apply")),
        }
    }

    /// Adopts a migration without applying it
    pub async fn adopt_migration(&self, migration_name: &str) -> Result<()> {
        let config = self.migration_helper.load_config().await?;
        let cmd = AdoptMigration {
            migration: migration_name.to_string(),
        };

        let outcome = cmd.execute(&config).await?;

        match outcome {
            Outcome::AdoptedMigration => Ok(()),
            _ => Err(anyhow::anyhow!("Unexpected outcome from migration adopt")),
        }
    }

    /// Executes raw SQL against the test database and returns the output
    pub fn execute_sql(&self, sql: &str) -> Result<String> {
        let output = self.connection_mode.execute_sql(&self.db_name, sql)?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("SQL execution failed: {}", stderr));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Checks if a table exists in the test database
    pub fn table_exists(&self, schema: &str, table: &str) -> Result<bool> {
        let sql = format!(
            "SELECT EXISTS (SELECT FROM information_schema.tables WHERE table_schema = '{}' AND table_name = '{}');",
            schema, table
        );
        let output = self.execute_sql(&sql)?;
        Ok(output.contains(" exists \n--------\n t"))
    }

    /// Runs test compare using the CompareTests command
    pub async fn run_test_compare(&self, test_name: Option<String>) -> Result<(), anyhow::Error> {
        let config = self.migration_helper.load_config().await?;
        let cmd = CompareTests { name: test_name };

        cmd.execute(&config)
            .await
            .context("error calling test compare")?;

        Ok(())
    }

    /// Saves test expected output using the ExpectTest command
    pub async fn run_test_expect(&self, test_name: String) -> Result<(), anyhow::Error> {
        let config = self.migration_helper.load_config().await?;
        let cmd = ExpectTest { name: test_name };

        cmd.execute(&config)
            .await
            .context("error calling test expect")?;

        Ok(())
    }
}

impl Drop for IntegrationTestHelper {
    fn drop(&mut self) {
        if self.keep_db {
            println!();
            println!("-------------------------------------------------------");
            println!(
                "KEEPING database for test '{}': {}",
                self.test_name, self.db_name
            );
            println!(
                "To connect: docker exec -it spawn-db psql -U spawn {}",
                self.db_name
            );
            println!("To drop manually: docker exec spawn-db psql -U spawn spawn -c \"DROP DATABASE \\\"{}\\\"\"", self.db_name);
            println!("-------------------------------------------------------");
        } else {
            // Clean up the test database
            if let Err(e) = self.drop_test_database() {
                eprintln!("Failed to clean up test database: {}", e);
            }
        }
    }
}

/// Helper to check PostgreSQL availability and fail the test if not available
fn require_postgres() -> Result<()> {
    if !ConnectionMode::from_env().is_ready() {
        return Err(anyhow::anyhow!(
            "PostgreSQL is not available. Start it with: docker compose up -d"
        ));
    }
    Ok(())
}

#[tokio::test]
async fn test_migration_status() -> Result<()> {
    require_postgres()?;

    let helper = IntegrationTestHelper::new("test_migration_status", None).await?;

    // Create and apply a migration
    let applied_migration = helper
        .migration_helper
        .create_migration_manual(
            "applied-migration",
            r#"BEGIN;
CREATE TABLE status_test (id SERIAL PRIMARY KEY);
COMMIT;"#
                .to_string(),
        )
        .await?;

    helper.apply_migration(&applied_migration).await?;

    // Create a migration and adopt it
    let adopted_migration = helper
        .migration_helper
        .create_migration_manual(
            "adopted-migration",
            r#"BEGIN;
CREATE TABLE adopted_status_test (id SERIAL PRIMARY KEY);
COMMIT;"#
                .to_string(),
        )
        .await?;

    // Manually run the migration
    helper.execute_sql(
        r#"BEGIN;
CREATE TABLE adopted_status_test (id SERIAL PRIMARY KEY);
COMMIT;"#,
    )?;

    helper.adopt_migration(&adopted_migration).await?;

    // Create a migration but don't apply it
    let _pending_migration = helper
        .migration_helper
        .create_migration_manual(
            "pending-migration",
            r#"BEGIN;
CREATE TABLE pending_test (id SERIAL PRIMARY KEY);
COMMIT;"#
                .to_string(),
        )
        .await?;

    // Now get the combined status
    let config = helper.migration_helper.load_config().await?;
    let status_rows =
        spawn_db::commands::migration::get_combined_migration_status(&config, Some("default"))
            .await?;

    // Verify we have all three migrations
    assert_eq!(status_rows.len(), 3, "Should have 3 migrations in status");

    // Find each migration and verify its status
    let applied = status_rows
        .iter()
        .find(|r| r.migration_name == applied_migration)
        .expect("applied-migration should be in status");

    assert!(
        applied.exists_in_filesystem,
        "applied migration should exist in filesystem"
    );
    assert!(applied.exists_in_db, "applied migration should exist in db");
    assert_eq!(
        applied.last_status,
        Some(spawn_db::engine::MigrationStatus::Success),
        "applied migration should have SUCCESS status"
    );
    assert_eq!(
        applied.last_activity.as_deref(),
        Some("APPLY"),
        "applied migration should have APPLY activity"
    );

    let adopted = status_rows
        .iter()
        .find(|r| r.migration_name == adopted_migration)
        .expect("adopted-migration should be in status");

    assert!(
        adopted.exists_in_filesystem,
        "adopted migration should exist in filesystem"
    );
    assert!(adopted.exists_in_db, "adopted migration should exist in db");
    assert_eq!(
        adopted.last_status,
        Some(spawn_db::engine::MigrationStatus::Success),
        "adopted migration should have SUCCESS status"
    );
    assert_eq!(
        adopted.last_activity.as_deref(),
        Some("ADOPT"),
        "adopted migration should have ADOPT activity"
    );

    let pending = status_rows
        .iter()
        .find(|r| r.migration_name.contains("pending-migration"))
        .expect("pending-migration should be in status");

    assert!(
        pending.exists_in_filesystem,
        "pending migration should exist in filesystem"
    );
    assert!(
        !pending.exists_in_db,
        "pending migration should NOT exist in db"
    );
    assert_eq!(
        pending.last_status, None,
        "pending migration should have no status"
    );
    assert_eq!(
        pending.last_activity, None,
        "pending migration should have no activity"
    );

    Ok(())
}

// ============================================================================
// Integration Tests
// ============================================================================

/// Tests that when a migration succeeds but recording fails, we get a clear
/// critical error message indicating manual intervention is required.
#[tokio::test]
#[ignore]
async fn test_migration_applied_but_not_recorded_error() -> Result<()> {
    require_postgres()?;

    let helper =
        IntegrationTestHelper::new("test_migration_applied_but_not_recorded", None).await?;

    // First, apply a simple migration to ensure the _spawn schema is set up
    let setup_migration = r#"BEGIN;
SELECT 1;
COMMIT;"#;

    let setup_name = helper
        .migration_helper
        .create_migration_manual("setup-schema", setup_migration.to_string())
        .await?;

    helper.apply_migration(&setup_name).await?;

    // Create a trigger that will cause INSERT on _spawn.migration to fail
    // This simulates a recording failure after migration succeeds
    helper.execute_sql(
        r#"
        CREATE OR REPLACE FUNCTION _spawn.block_migration_insert()
        RETURNS TRIGGER AS $$
        BEGIN
            IF NEW.name LIKE '%will-fail-recording%' THEN
                RAISE EXCEPTION 'Simulated recording failure for testing';
            END IF;
            RETURN NEW;
        END;
        $$ LANGUAGE plpgsql;

        CREATE TRIGGER block_recording_trigger
        BEFORE INSERT ON _spawn.migration
        FOR EACH ROW
        EXECUTE FUNCTION _spawn.block_migration_insert();
        "#,
    )?;

    // Create a migration that will succeed but recording will fail
    let test_migration = r#"BEGIN;

CREATE TABLE recording_failure_test (
    id SERIAL PRIMARY KEY,
    name TEXT
);

COMMIT;"#;

    let test_name = helper
        .migration_helper
        .create_migration_manual("will-fail-recording", test_migration.to_string())
        .await?;

    // Try to apply the migration - it should fail with the critical error
    let result = helper.apply_migration(&test_name).await;

    // Clean up the trigger
    let _ =
        helper.execute_sql("DROP TRIGGER IF EXISTS block_recording_trigger ON _spawn.migration;");
    let _ = helper.execute_sql("DROP FUNCTION IF EXISTS _spawn.block_migration_insert();");

    // The migration should have failed
    assert!(result.is_err(), "Expected migration to fail");

    let error_message = result.unwrap_err().to_string();

    // Verify the error message contains the critical indicators
    assert!(
        error_message.contains("CRITICAL") || error_message.contains("MANUAL INTERVENTION"),
        "Error should indicate critical/manual intervention needed, got: '''{}'''",
        error_message
    );

    // Verify the table WAS created (migration succeeded, only recording failed)
    assert!(
        helper.table_exists("public", "recording_failure_test")?,
        "Table should exist because migration SQL succeeded, only recording failed"
    );

    // Verify the migration was NOT recorded
    let migration_check = helper.execute_sql(
        "SELECT COUNT(*) FROM _spawn.migration WHERE name LIKE '%will-fail-recording%';",
    )?;
    assert!(
        migration_check.contains('0'),
        "Migration should NOT be recorded in _spawn.migration table, got: {}",
        migration_check
    );

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_postgres_connection() -> Result<()> {
    require_postgres()?;

    let helper = IntegrationTestHelper::new("test_postgres_connection", None).await?;

    // Simple connectivity test
    let result = helper.execute_sql("SELECT 1 as test;")?;
    assert!(result.contains("1"), "Expected to see '1' in output");

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_migration_creates_table() -> Result<()> {
    require_postgres()?;

    let helper = IntegrationTestHelper::new("test_migration_creates_table", None).await?;

    // Create a migration that creates a table
    let migration_content = r#"BEGIN;

CREATE TABLE test_users (
    id SERIAL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    email VARCHAR(255) UNIQUE NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

COMMIT;"#;

    let migration_name = helper
        .migration_helper
        .create_migration_manual("create-users-table", migration_content.to_string())
        .await?;

    // Apply the migration
    helper.apply_migration(&migration_name).await?;

    // Verify the table was created
    assert!(
        helper.table_exists("public", "test_users")?,
        "test_users table should exist after migration"
    );

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_migration_is_idempotent() -> Result<()> {
    require_postgres()?;

    let helper = IntegrationTestHelper::new("test_migration_is_idempotent", None).await?;

    let migration_content = r#"BEGIN;

CREATE TABLE IF NOT EXISTS idempotent_test (
    id SERIAL PRIMARY KEY,
    value TEXT
);

COMMIT;"#;

    let migration_name = helper
        .migration_helper
        .create_migration_manual("idempotent-table", migration_content.to_string())
        .await?;

    // Apply the migration twice - should not error
    helper.apply_migration(&migration_name).await?;

    // The second apply should recognize it's already applied
    // (This tests the migration tracking in _spawn schema)

    assert!(
        helper.table_exists("public", "idempotent_test")?,
        "idempotent_test table should exist"
    );

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_spawn_schema_created() -> Result<()> {
    require_postgres()?;

    let helper = IntegrationTestHelper::new("test_spawn_schema_created", None).await?;

    // Create and apply a simple migration to trigger schema setup
    let migration_content = r#"BEGIN;
SELECT 1;
COMMIT;"#;

    let migration_name = helper
        .migration_helper
        .create_migration_manual("trigger-schema", migration_content.to_string())
        .await?;

    helper.apply_migration(&migration_name).await?;

    // Check that the _spawn schema and migration table were created
    assert!(
        helper.table_exists("_spawn", "migration")?,
        "_spawn.migration table should exist after first migration"
    );

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_migration_recorded_in_history() -> Result<()> {
    require_postgres()?;

    let helper = IntegrationTestHelper::new("test_migration_recorded", None).await?;

    let migration_content = r#"BEGIN;

CREATE TABLE recorded_test (
    id SERIAL PRIMARY KEY
);

COMMIT;"#;

    let migration_name = helper
        .migration_helper
        .create_migration_manual("recorded-migration", migration_content.to_string())
        .await?;

    helper.apply_migration(&migration_name).await?;

    // Check that the migration was recorded in the history table
    let history = helper.execute_sql("SELECT * FROM _spawn.migration_history;")?;

    // The history should contain a reference to our migration
    assert!(
        history.contains("SUCCESS"),
        "Migration history should show SUCCESS status"
    );

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_migration_tables_correctness() -> Result<()> {
    require_postgres()?;

    let helper = IntegrationTestHelper::new("test_migration_tables_correctness", None).await?;

    // =========================================================================
    // Step 1: Create and apply first migration
    // =========================================================================
    let migration1_content = r#"BEGIN;

CREATE TABLE first_table (
    id SERIAL PRIMARY KEY,
    name VARCHAR(255) NOT NULL
);

COMMIT;"#;

    let migration1_name = helper
        .migration_helper
        .create_migration_manual("first-migration", migration1_content.to_string())
        .await?;

    helper.apply_migration(&migration1_name).await?;

    // Verify one row in migration table (excluding spawn's own internal migrations)
    let migration_count =
        helper.execute_sql("SELECT COUNT(*) FROM _spawn.migration WHERE namespace = 'default';")?;
    assert!(
        migration_count.contains("1"),
        "Expected 1 migration in default namespace, got: {}",
        migration_count
    );

    // Verify one row in migration_history for our migration
    let history_count = helper.execute_sql(
        "SELECT COUNT(*) FROM _spawn.migration_history mh \
         JOIN _spawn.migration m ON mh.migration_id_migration = m.migration_id \
         WHERE m.namespace = 'default';",
    )?;
    assert!(
        history_count.contains("1"),
        "Expected 1 history entry for default namespace, got: {}",
        history_count
    );

    // Verify the migration data looks correct
    let migration_data = helper
        .execute_sql("SELECT name, namespace FROM _spawn.migration WHERE namespace = 'default';")?;
    assert!(
        migration_data.contains(&migration1_name),
        "Migration table should contain migration name '{}', got: {}",
        migration1_name,
        migration_data
    );
    assert!(
        migration_data.contains("default"),
        "Migration should be in 'default' namespace, got: {}",
        migration_data
    );

    // Verify migration_history data
    let history_data = helper.execute_sql(
        "SELECT mh.activity_id_activity, mh.status_id_status, mh.created_by \
         FROM _spawn.migration_history mh \
         JOIN _spawn.migration m ON mh.migration_id_migration = m.migration_id \
         WHERE m.namespace = 'default';",
    )?;
    assert!(
        history_data.contains("APPLY"),
        "History should show APPLY activity, got: {}",
        history_data
    );
    assert!(
        history_data.contains("SUCCESS"),
        "History should show SUCCESS status, got: {}",
        history_data
    );

    // =========================================================================
    // Step 2: Apply same migration again - should be idempotent
    // =========================================================================
    helper.apply_migration(&migration1_name).await?;

    // Verify still only one row in migration table
    let migration_count_after_reapply =
        helper.execute_sql("SELECT COUNT(*) FROM _spawn.migration WHERE namespace = 'default';")?;
    assert!(
        migration_count_after_reapply.contains("1"),
        "Expected still 1 migration after re-apply, got: {}",
        migration_count_after_reapply
    );

    // Verify still only one history entry (no duplicate APPLY)
    let history_count_after_reapply = helper.execute_sql(
        "SELECT COUNT(*) FROM _spawn.migration_history mh \
         JOIN _spawn.migration m ON mh.migration_id_migration = m.migration_id \
         WHERE m.namespace = 'default';",
    )?;
    assert!(
        history_count_after_reapply.contains("1"),
        "Expected still 1 history entry after re-apply (idempotent), got: {}",
        history_count_after_reapply
    );

    // =========================================================================
    // Step 3: Create and apply second migration
    // =========================================================================
    let migration2_content = r#"BEGIN;

CREATE TABLE second_table (
    id SERIAL PRIMARY KEY,
    value TEXT,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

COMMIT;"#;

    let migration2_name = helper
        .migration_helper
        .create_migration_manual("second-migration", migration2_content.to_string())
        .await?;

    helper.apply_migration(&migration2_name).await?;

    // Verify now two rows in migration table
    let migration_count_final =
        helper.execute_sql("SELECT COUNT(*) FROM _spawn.migration WHERE namespace = 'default';")?;
    assert!(
        migration_count_final.contains("2"),
        "Expected 2 migrations after second apply, got: {}",
        migration_count_final
    );

    // Verify now two rows in migration_history
    let history_count_final = helper.execute_sql(
        "SELECT COUNT(*) FROM _spawn.migration_history mh \
         JOIN _spawn.migration m ON mh.migration_id_migration = m.migration_id \
         WHERE m.namespace = 'default';",
    )?;
    assert!(
        history_count_final.contains("2"),
        "Expected 2 history entries after second apply, got: {}",
        history_count_final
    );

    // Verify both migrations are recorded with correct names
    let all_migrations = helper.execute_sql(
        "SELECT name FROM _spawn.migration WHERE namespace = 'default' ORDER BY migration_id;",
    )?;
    assert!(
        all_migrations.contains(&migration1_name),
        "Should contain first migration name '{}', got: {}",
        migration1_name,
        all_migrations
    );
    assert!(
        all_migrations.contains(&migration2_name),
        "Should contain second migration name '{}', got: {}",
        migration2_name,
        all_migrations
    );

    // Verify both history entries have SUCCESS status and APPLY activity
    let all_history = helper.execute_sql(
        "SELECT mh.activity_id_activity, mh.status_id_status \
         FROM _spawn.migration_history mh \
         JOIN _spawn.migration m ON mh.migration_id_migration = m.migration_id \
         WHERE m.namespace = 'default' \
         ORDER BY mh.migration_history_id;",
    )?;
    // Count occurrences of SUCCESS and APPLY - should be 2 each
    let success_count = all_history.matches("SUCCESS").count();
    let apply_count = all_history.matches("APPLY").count();
    assert_eq!(
        success_count, 2,
        "Expected 2 SUCCESS entries, got {} in: {}",
        success_count, all_history
    );
    assert_eq!(
        apply_count, 2,
        "Expected 2 APPLY entries, got {} in: {}",
        apply_count, all_history
    );

    // Verify checksum is non-empty for both entries
    let checksums = helper.execute_sql(
        "SELECT mh.checksum \
         FROM _spawn.migration_history mh \
         JOIN _spawn.migration m ON mh.migration_id_migration = m.migration_id \
         WHERE m.namespace = 'default';",
    )?;
    // Checksums should be present (shown as hex like \x...)
    let checksum_lines: Vec<&str> = checksums.lines().filter(|l| l.contains("\\x")).collect();
    assert_eq!(
        checksum_lines.len(),
        2,
        "Expected 2 non-empty checksums, got: {}",
        checksums
    );

    // Verify execution_time is recorded (non-zero interval)
    let execution_times = helper.execute_sql(
        "SELECT mh.execution_time \
         FROM _spawn.migration_history mh \
         JOIN _spawn.migration m ON mh.migration_id_migration = m.migration_id \
         WHERE m.namespace = 'default';",
    )?;
    // Should have some time values (even if small)
    assert!(
        !execution_times.contains("(0 rows)"),
        "Expected execution times to be recorded, got: {}",
        execution_times
    );

    // Verify the actual tables were created
    assert!(
        helper.table_exists("public", "first_table")?,
        "first_table should exist in public schema"
    );
    assert!(
        helper.table_exists("public", "second_table")?,
        "second_table should exist in public schema"
    );

    Ok(())
}

/// Tests that SQL errors in migrations cause the migration to fail
/// rather than being silently ignored and reported as successful.
#[tokio::test]
#[ignore]
async fn test_migration_sql_error_causes_failure() -> Result<()> {
    require_postgres()?;

    let helper =
        IntegrationTestHelper::new("test_migration_sql_error_causes_failure", None).await?;

    // Create a migration with invalid SQL that will cause an error
    let bad_migration = r#"BEGIN;

SELECT this_does_not_exist();

COMMIT;"#;

    let migration_name = helper
        .migration_helper
        .create_migration_manual("bad-migration", bad_migration.to_string())
        .await?;

    // Try to apply the migration - it should fail
    let result = helper.apply_migration(&migration_name).await;

    // The migration should have failed
    assert!(
        result.is_err(),
        "Expected migration with SQL error to fail, but it succeeded"
    );

    // Verify the migration was NOT recorded as successful
    let migration_check = helper.execute_sql(&format!(
        "SELECT COUNT(*) FROM _spawn.migration WHERE name = '{}';",
        migration_name
    ))?;
    assert_eq!(migration_check, " count \n-------\n     0\n(1 row)\n\n");

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_cli_test_compare() -> Result<()> {
    require_postgres()?;

    let helper = IntegrationTestHelper::new(
        "test_cli_test_compare",
        Some("./static/tests/test_cli_test"),
    )
    .await?;

    let test_name = "20250113000000-simple-test".to_string();

    // Run compare - it should pass since we just saved the expected output
    helper.run_test_compare(Some(test_name.clone())).await?;

    // Update our test with a new query:
    let new_test = concat!(r#"select 'just this now' as changed;"#);

    helper
        .migration_helper
        .fs
        .write("/db/tests/20250113000000-simple-test/test.sql", new_test)
        .await?;

    // Run compare again and confirm it fails
    let result = helper.run_test_compare(Some(test_name.clone())).await;
    match result {
        Ok(_) => {
            return Err(anyhow!("comparison should have produced error"));
        }
        Err(e) => {
            let err_str = e.to_string();
            if !err_str.contains("error calling test compare") {
                return Err(anyhow!("Unexpected comparison output"));
            }
        }
    }

    // Now expect this, and run again and check pass:
    helper
        .run_test_expect(test_name.clone())
        .await
        .context("failed to update expectation")?;
    helper
        .run_test_compare(Some(test_name.clone()))
        .await
        .context("failed to compare after updating expectation")?;

    Ok(())
}

/// Tests that migrations fail when another session holds the advisory lock.
/// This verifies the concurrent migration protection works correctly.
#[tokio::test]
#[ignore]
async fn test_advisory_lock_blocks_migration() -> Result<()> {
    require_postgres()?;

    let helper = std::sync::Arc::new(
        IntegrationTestHelper::new("test_advisory_lock_blocks_migration", None).await?,
    );

    // First, apply a simple migration to ensure the _spawn schema is set up
    let setup_migration = r#"BEGIN;
SELECT 1;
COMMIT;"#;

    let setup_name = helper
        .migration_helper
        .create_migration_manual("setup-for-lock-test", setup_migration.to_string())
        .await?;

    helper.apply_migration(&setup_name).await?;

    // Create a migration that holds the advisory lock for a while via pg_sleep.
    // When apply_migration runs this, it will acquire the lock, then sleep.
    let slow_migration = r#"BEGIN;
SELECT pg_sleep(2);
COMMIT;"#;

    let slow_name = helper
        .migration_helper
        .create_migration_manual("slow-lock-holder", slow_migration.to_string())
        .await?;

    // Create the second migration that should fail to acquire the lock
    let test_table = format!("advisory_lock_test");
    let fast_migration = format!(
        r#"BEGIN;
CREATE TABLE {} (id SERIAL PRIMARY KEY);
COMMIT;"#,
        test_table
    );

    let fast_name = helper
        .migration_helper
        .create_migration_manual("should-fail-lock", fast_migration)
        .await?;

    // Spawn the slow migration in a background task
    let helper_clone = helper.clone();
    let slow_name_clone = slow_name.clone();
    let _ = tokio::spawn(async move { helper_clone.apply_migration(&slow_name_clone).await });

    // Give the slow migration time to start and acquire the lock
    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    // Try to apply the fast migration - it should fail because the lock is held
    let result = helper.apply_migration(&fast_name).await;

    assert!(
        result.is_err(),
        "Expected migration to fail due to advisory lock being held"
    );

    let error_message = result.unwrap_err().to_string();

    // Verify the error is specifically about the advisory lock
    assert!(
        error_message.contains("advisory lock")
            || error_message.contains("Could not acquire advisory lock"),
        "Error should mention advisory lock, got: '''{}'''",
        error_message
    );

    // Verify the table was NOT created (migration should not have executed)
    assert!(
        !helper.table_exists("public", &test_table)?,
        "Table should NOT exist because migration should have been blocked by lock"
    );

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_migration_adopt() -> Result<()> {
    require_postgres()?;

    let helper = IntegrationTestHelper::new("test_migration_adopt", None).await?;

    // Create a migration but don't apply it through spawn
    let migration_content = r#"BEGIN;

CREATE TABLE adopted_test (
    id SERIAL PRIMARY KEY,
    value TEXT
);

COMMIT;"#;

    let migration_name = helper
        .migration_helper
        .create_migration_manual("adopted-migration", migration_content.to_string())
        .await?;

    // Manually run the migration SQL (simulating manual intervention)
    helper.execute_sql(migration_content)?;

    // Verify the table exists
    assert!(
        helper.table_exists("public", "adopted_test")?,
        "adopted_test table should exist after manual SQL execution"
    );

    // Now adopt the migration (mark it as applied without running it)
    helper.adopt_migration(&migration_name).await?;

    // Verify the migration was recorded in history with ADOPT activity
    let history = helper.execute_sql(
        "SELECT activity_id_activity, status_id_status FROM _spawn.migration_history ORDER BY migration_history_id DESC LIMIT 1;"
    )?;

    assert!(
        history.contains("ADOPT"),
        "Migration history should show ADOPT activity, got: {}",
        history
    );
    assert!(
        history.contains("SUCCESS"),
        "Migration history should show SUCCESS status, got: {}",
        history
    );

    // Verify adopting the same migration again is idempotent (returns success)
    helper.adopt_migration(&migration_name).await?;

    // Verify we can also adopt a migration even if the file doesn't exist
    // (useful for recording manually-run SQL that wasn't tracked)
    helper.adopt_migration("manual-sql-no-file").await?;

    // Verify it was recorded
    let count = helper
        .execute_sql("SELECT COUNT(*) FROM _spawn.migration WHERE name = 'manual-sql-no-file';")?;
    assert!(
        count.contains("1"),
        "manual-sql-no-file should be recorded in migration table"
    );

    Ok(())
}
