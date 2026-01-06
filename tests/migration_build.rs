use anyhow::Result;
use futures::TryStreamExt;
use opendal::services::Memory;
use opendal::Operator;
use pretty_assertions::assert_eq;
use spawn::{
    cli::{run_cli, Cli, Commands, MigrationCommands, Outcome},
    config::Config,
};
use tokio;

/// Expected default new migration content:
const DEFAULT_MIGRATION_CONTENT: &str = r#"BEGIN;

COMMIT;
"#;

/// Reusable test helper structure for setting up migration tests
pub struct MigrationTestHelper {
    fs: Operator,
}

impl MigrationTestHelper {
    fn config_path(&self) -> String {
        "./spawn.toml".to_string()
    }
}

impl MigrationTestHelper {
    pub async fn load_config(&self) -> Result<Config> {
        Config::load(&self.config_path(), &self.fs, None).await
    }

    /// Creates a new test environment with basic directory structure and config
    pub async fn new() -> Result<Self> {
        let mem_service = Memory::default();
        let mem_op = Operator::new(mem_service)?.finish();

        let mth = Self { fs: mem_op.clone() };

        // Create a test config file
        let config_content = format!(
            r#"
spawn_folder = "./db"
database = "postgres_psql"

[databases.postgres_psql]
spawn_database = "spawn"
engine = "postgres-psql"
command = ["docker", "exec", "-i", "spawn-db", "psql", "-U", "spawn", "spawn"]
"#,
        );
        mem_op.write(&mth.config_path(), config_content).await?;

        Ok(mth)
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
        let outcome: Outcome = run_cli(cli, &self.fs).await?;

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
        script_content: String,
    ) -> Result<String, anyhow::Error> {
        let migration_name = &self.create_migration(name).await?;
        let cfg = self.load_config().await?;

        // Replace the content of the migration file with the provided script content
        let migration_path = cfg.migration_script_file_path(&migration_name);
        self.fs.write(&migration_path, script_content).await?;

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
                    migration: migration_name.to_string(),
                    pinned,
                    variables: None,
                }),
                environment: None,
            }),
        };

        let outcome: Outcome = run_cli(cli, &self.fs).await?;

        match outcome {
            Outcome::BuiltMigration { content } => Ok(content),
            _ => Err(anyhow::anyhow!("Unexpected outcome")),
        }
    }

    pub async fn list_fs_contents(&self, label: &str) -> Result<()> {
        let mut lister = self.fs.lister_with(".").recursive(true).await?;

        println!("listing files for '{}'", label);
        while let Some(entry) = lister.try_next().await? {
            let file_data = self.fs.read(&entry.path()).await?.to_bytes();
            println!("(len {}). found {}", file_data.len(), entry.path());
        }

        Ok(())
    }
}

// Run a create migration test:
#[tokio::test]
async fn test_create_migration() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new().await?;

    // Test that we can create a migration
    let migration_name = helper
        .create_migration("test-create")
        .await
        .expect("Failed to create migration with helper");

    let cfg = helper.load_config().await?;

    // Check that <migration folder>/up.sql exists:
    let script_path = format!("{}/up.sql", cfg.migration_folder(&migration_name));

    // Check the contents are what we expect:
    let file_data = helper.fs.read(&script_path).await?.to_bytes();
    let file_contents = String::from_utf8(file_data.to_vec())?;
    assert_eq!(DEFAULT_MIGRATION_CONTENT, file_contents,);

    Ok(())
}

#[tokio::test]
async fn test_migration_build_basic() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new().await?;

    // Create a simple migration script
    let script_content = r#"BEGIN;

CREATE TABLE users (
    id SERIAL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    email VARCHAR(255) UNIQUE NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

COMMIT;"#;

    let migration_name = helper
        .create_migration_manual("test-migration-build-basic", script_content.to_string())
        .await?;

    // Build the migration
    let built = helper.build_migration(&migration_name, false).await?;
    assert_eq!(script_content, built);

    Ok(())
}

#[tokio::test]
async fn test_migration_build_with_component() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new().await?;

    // Create a simple migration script
    let script_content = r#"BEGIN;

CREATE TABLE users (
    id SERIAL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    email VARCHAR(255) UNIQUE NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

COMMIT;"#;

    let migration_name = helper
        .create_migration_manual("test-migration-build-basic", script_content.to_string())
        .await?;

    // Build the migration
    let built = helper.build_migration(&migration_name, false).await?;
    assert_eq!(script_content, built);

    Ok(())
}
