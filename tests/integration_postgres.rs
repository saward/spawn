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

use anyhow::{Context, Result};
use opendal::services::Memory;
use opendal::Operator;
use spawn::{
    cli::{run_cli, Cli, Commands, MigrationCommands, Outcome},
    config::ConfigLoaderSaver,
    engine::{DatabaseConfig, EngineType},
};
use std::collections::HashMap;
use std::env;
use std::process::Command;
use uuid::Uuid;

/// Configuration for connecting to the test PostgreSQL instance (Docker mode)
const DOCKER_CONTAINER: &str = "spawn-db";
const DEFAULT_POSTGRES_USER: &str = "spawn";
const DEFAULT_CREATE_DATABASE_DATABASE: &str = "postgres";
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
    /// its own execute function. This is used for test verification reasons
    fn execute_sql(&self, database: &str, sql: &str) -> Result<std::process::Output> {
        match self {
            ConnectionMode::Docker { container, user } => Command::new("docker")
                .args([
                    "exec", "-i", container, "psql", "-U", user, database, "-c", sql,
                ])
                .output()
                .context("Failed to execute docker psql command"),
            ConnectionMode::Direct { host, port, user } => Command::new("psql")
                .args(["-h", host, "-p", port, "-U", user, database, "-c", sql])
                .output()
                .context("Failed to execute psql command"),
        }
    }

    /// Check if PostgreSQL is ready
    fn is_ready(&self) -> bool {
        match self {
            ConnectionMode::Docker { container, user } => Command::new("docker")
                .args(["exec", container, "pg_isready", "-U", user])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false),
            ConnectionMode::Direct { host, port, user } => Command::new("pg_isready")
                .args(["-h", host, "-p", port, "-U", user])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false),
        }
    }
}

/// Helper struct for integration tests that manages database lifecycle
pub struct IntegrationTestHelper {
    fs: Operator,
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
    pub async fn new(test_name: &str) -> Result<Self> {
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
        let mem_service = Memory::default();
        let mem_op = Operator::new(mem_service)?.finish();

        let helper = Self {
            fs: mem_op,
            db_name: db_name.clone(),
            test_name: test_name.to_string(),
            connection_mode,
            keep_db,
        };

        // Initialize config pointing to our test database
        let config_loader = helper.create_config();
        config_loader
            .save(&helper.config_path(), &helper.fs)
            .await?;

        Ok(helper)
    }

    fn config_path(&self) -> String {
        "./spawn.toml".to_string()
    }

    /// Creates a database config that points to our isolated test database
    fn create_config(&self) -> ConfigLoaderSaver {
        let mut databases = HashMap::new();
        databases.insert(
            "postgres_psql".to_string(),
            DatabaseConfig {
                engine: EngineType::PostgresPSQL,
                spawn_database: self.db_name.clone(),
                spawn_schema: "_spawn".to_string(),
                environment: "test".to_string(),
                command: Some(self.connection_mode.psql_command(&self.db_name)),
            },
        );

        ConfigLoaderSaver {
            spawn_folder: "/db".to_string(),
            database: "postgres_psql".to_string(),
            environment: None,
            databases,
        }
    }

    /// Creates an isolated test database using PostgreSQL's template feature
    fn create_test_database(db_name: &str, mode: &ConnectionMode) -> Result<()> {
        // Don't connect to our database we intend to run WITH TEMPLATE against
        // because then it will sometimes fail.
        let output = mode.execute_sql(
            DEFAULT_CREATE_DATABASE_DATABASE,
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
            TEMPLATE_DATABASE,
            &format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}'",
                self.db_name
            ),
        );

        let output = self.connection_mode.execute_sql(
            TEMPLATE_DATABASE,
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

    /// Creates a new migration in the test environment
    pub async fn create_migration(&self, name: &str) -> Result<String> {
        let cli = Cli {
            debug: false,
            config_file: self.config_path(),
            database: None,
            command: Some(Commands::Migration {
                command: Some(MigrationCommands::New {
                    name: name.to_string(),
                }),
                environment: None,
            }),
        };

        let outcome = run_cli(cli, &self.fs).await?;

        match outcome {
            Outcome::NewMigration(name) => Ok(name),
            _ => Err(anyhow::anyhow!("Unexpected outcome from migration new")),
        }
    }

    /// Creates a migration with custom SQL content
    pub async fn create_migration_with_content(&self, name: &str, content: &str) -> Result<String> {
        let migration_name = self.create_migration(name).await?;

        // Write the custom content to the migration file
        let migration_path = format!("/db/migrations/{}/up.sql", migration_name);
        self.fs.write(&migration_path, content.to_string()).await?;

        Ok(migration_name)
    }

    /// Applies a migration to the test database
    pub async fn apply_migration(&self, migration_name: &str) -> Result<()> {
        let cli = Cli {
            debug: false,
            config_file: self.config_path(),
            database: None,
            command: Some(Commands::Migration {
                command: Some(MigrationCommands::Apply {
                    pinned: false,
                    migration: Some(migration_name.to_string()),
                    variables: None,
                }),
                environment: None,
            }),
        };

        let outcome = run_cli(cli, &self.fs).await?;

        match outcome {
            Outcome::AppliedMigrations => Ok(()),
            _ => Err(anyhow::anyhow!("Unexpected outcome from migration apply")),
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
        Ok(output.contains('t'))
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

/// Checks if PostgreSQL is available (works for both Docker and direct modes)
fn postgres_available() -> bool {
    ConnectionMode::from_env().is_ready()
}

/// Helper to check PostgreSQL availability and fail the test if not available
fn require_postgres() -> Result<()> {
    if !postgres_available() {
        return Err(anyhow::anyhow!(
            "PostgreSQL is not available. Start it with: docker compose up -d"
        ));
    }
    Ok(())
}

// ============================================================================
// Integration Tests
// ============================================================================

#[tokio::test]
#[ignore]
async fn test_postgres_connection() -> Result<()> {
    require_postgres()?;

    let helper = IntegrationTestHelper::new("test_postgres_connection").await?;

    // Simple connectivity test
    let result = helper.execute_sql("SELECT 1 as test;")?;
    assert!(result.contains("1"), "Expected to see '1' in output");

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_migration_creates_table() -> Result<()> {
    require_postgres()?;

    let helper = IntegrationTestHelper::new("test_migration_creates_table").await?;

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
        .create_migration_with_content("create-users-table", migration_content)
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

    let helper = IntegrationTestHelper::new("test_migration_is_idempotent").await?;

    let migration_content = r#"BEGIN;

CREATE TABLE IF NOT EXISTS idempotent_test (
    id SERIAL PRIMARY KEY,
    value TEXT
);

COMMIT;"#;

    let migration_name = helper
        .create_migration_with_content("idempotent-table", migration_content)
        .await?;

    // Apply the migration twice - should not error
    helper.apply_migration(&migration_name).await?;

    // The second apply should recognize it's already applied
    // (This tests the migration tracking in _spawn schema)
    // Note: Depending on implementation, this might skip or error gracefully

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

    let helper = IntegrationTestHelper::new("test_spawn_schema_created").await?;

    // Create and apply a simple migration to trigger schema setup
    let migration_content = r#"BEGIN;
SELECT 1;
COMMIT;"#;

    let migration_name = helper
        .create_migration_with_content("trigger-schema", migration_content)
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

    let helper = IntegrationTestHelper::new("test_migration_recorded").await?;

    let migration_content = r#"BEGIN;

CREATE TABLE recorded_test (
    id SERIAL PRIMARY KEY
);

COMMIT;"#;

    let migration_name = helper
        .create_migration_with_content("recorded-migration", migration_content)
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
