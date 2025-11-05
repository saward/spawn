use object_store::path::Path;
use pretty_assertions::assert_eq;
use spawn::cli::{run_cli, Cli, Commands, MigrationCommands, Outcome};
use std::fs;
use tempfile::TempDir;
use tokio;

/// Expected default new migration content:
const DEFAULT_MIGRATION_CONTENT: &str = r#"BEGIN;

COMMIT;
"#;

/// Reusable test helper structure for setting up migration tests
pub struct MigrationTestHelper {
    pub temp_dir: TempDir,
}

impl MigrationTestHelper {
    fn base_path(&self) -> String {
        // Returns static db folder:
        format!("{}/db", self.temp_dir.path().display())
    }

    fn migrations_dir(&self) -> String {
        format!("{}/migrations", self.base_path())
    }

    fn migration_script_path(&self, full_migration_name: &str) -> String {
        format!("{}/{}/up.sql", self.migrations_dir(), full_migration_name)
    }

    fn tests_dir(&self) -> String {
        format!("{}/tests", self.base_path())
    }

    fn components_dir(&self) -> String {
        format!("{}/components", self.base_path())
    }

    fn config_path(&self) -> String {
        format!("{}/spawn.toml", self.base_path())
    }
}

impl MigrationTestHelper {
    /// Creates a new test environment with basic directory structure and config
    pub fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");

        let mth = Self { temp_dir };

        // Create test directory structure
        fs::create_dir_all(mth.migrations_dir()).expect("Failed to create migrations dir");
        fs::create_dir_all(mth.components_dir()).expect("Failed to create components dir");
        fs::create_dir_all(mth.tests_dir()).expect("Failed to create tests dir");

        // Create a test config file
        let config_content = format!(
            r#"
spawn_folder = "{}/db"
database = "postgres_psql"

[databases.postgres_psql]
spawn_database = "spawn"
engine = "postgres-psql"
command = ["docker", "exec", "-i", "spawn-db", "psql", "-U", "spawn", "spawn"]
"#,
            mth.temp_dir.path().display(),
        );
        fs::write(&mth.config_path(), config_content).expect("Failed to write config file");

        mth
    }

    /// Creates a new migration using the CLI 'migration new' command
    pub async fn create_migration(&self, name: &str) -> Result<String, anyhow::Error> {
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

        // Run the CLI command to create migration
        let outcome: Outcome = run_cli(cli).await?;

        // Find the created migration directory
        let name = match outcome {
            Outcome::NewMigration(name) => name,
            _ => Err(anyhow::anyhow!(
                "Migration directory not found after creation"
            ))?,
        };

        Ok(name)
    }

    /// Creates a migration and then replaces the content of it with provided
    /// value:
    pub async fn create_migration_manual(
        &self,
        name: &str,
        script_content: &str,
    ) -> Result<String, anyhow::Error> {
        let migration_name = &self.create_migration(name).await?;

        // Replace the content of the migration file with the provided script content
        let migration_path = self.migration_script_path(&migration_name);
        fs::write(&migration_path, script_content)?;

        Ok(migration_name.clone())
    }

    /// Builds a migration using the CLI 'migration build' command
    pub async fn build_migration(
        &self,
        migration_name: &str,
        pinned: bool,
    ) -> Result<String, anyhow::Error> {
        let cli = Cli {
            debug: false,
            config_file: self.config_path(),
            database: None,
            command: Some(Commands::Migration {
                command: Some(MigrationCommands::Build {
                    migration: Path::from(migration_name),
                    pinned,
                    variables: None,
                }),
                environment: None,
            }),
        };

        let _outcome: Outcome = run_cli(cli).await?;

        // Note: In a real implementation, you'd need to capture stdout
        // For now, we'll return a placeholder indicating success
        Ok("Migration built successfully".to_string())
    }
}

// Run a create migration test:
#[tokio::test]
async fn test_create_migration() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new();

    // Test that we can create a migration
    let migration_name = helper
        .create_migration("test-create")
        .await
        .expect("Failed to create migration with helper");

    // Check that <migration folder>/up.sql exists:
    let script_path = format!("{}/{}/up.sql", helper.migrations_dir(), migration_name);
    assert!(
        std::path::Path::new(&script_path).exists(),
        "new migration script does not exist"
    );

    // Check script has expected input:
    let created_script_str = std::fs::read_to_string(&script_path)?;
    assert_eq!(DEFAULT_MIGRATION_CONTENT, created_script_str);

    Ok(())
}

#[tokio::test]
async fn test_migration_build_basic() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new();

    // Create a simple migration script
    let script_content = r#"BEGIN;

CREATE TABLE users (
    id SERIAL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    email VARCHAR(255) UNIQUE NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

// COMMIT;"#;

    let migration_name = helper
        .create_migration_manual("test-migration-build-basic", script_content)
        .await?;

    // Build the migration
    let result = helper.build_migration(&migration_name, false).await;
    assert!(result.is_ok());

    Ok(())
}
