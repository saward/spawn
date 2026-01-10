use anyhow::{Context, Result};
use futures::TryStreamExt;
use opendal::services::Memory;
use opendal::Operator;
use pretty_assertions::assert_eq;
use spawn::{
    cli::{run_cli, Cli, Commands, MigrationCommands, Outcome},
    config::{Config, ConfigLoaderSaver},
    engine::{DatabaseConfig, EngineType},
    store,
};
use std::collections::HashMap;
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

    /// Creates a new test environment with no data
    pub async fn new_empty() -> Result<Self> {
        let mem_service = Memory::default();
        let mem_op = Operator::new(mem_service)?.finish();

        Self::new_from_operator(mem_op).await
    }

    pub async fn new_from_local_folder(folder: &str) -> Result<Self> {
        let mem_op =
            store::disk_to_operator(folder, Some("/db/"), store::DesiredOperator::Memory).await?;

        Self::new_from_operator(mem_op).await
    }

    pub async fn new_from_operator(op: Operator) -> Result<Self> {
        let mth = Self { fs: op };

        let config_loader = Self::default_config_loadersaver();
        config_loader.save(&mth.config_path(), &mth.fs).await?;

        Ok(mth)
    }

    fn default_config_loadersaver() -> ConfigLoaderSaver {
        let mut databases = HashMap::new();
        databases.insert(
            "postgres_psql".to_string(),
            DatabaseConfig {
                engine: EngineType::PostgresPSQL,
                spawn_database: "spawn".to_string(),
                spawn_schema: "public".to_string(),
                environment: "dev".to_string(),
                command: Some(vec![
                    "docker".to_string(),
                    "exec".to_string(),
                    "-i".to_string(),
                    "spawn-db".to_string(),
                    "psql".to_string(),
                    "-U".to_string(),
                    "spawn".to_string(),
                    "spawn".to_string(),
                ]),
            },
        );

        ConfigLoaderSaver {
            spawn_folder: "/db".to_string(),
            database: "postgres_psql".to_string(),
            environment: None,
            databases,
        }
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
        let migration_path = cfg.pather().migration_script_file_path(&migration_name);
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

    /// Pins a migration
    pub async fn pin_migration(&self, migration_name: &str) -> Result<String, anyhow::Error> {
        let cli = Cli {
            debug: false,
            config_file: self.config_path(),
            database: None,
            command: Some(Commands::Migration {
                command: Some(MigrationCommands::Pin {
                    migration: migration_name.to_string(),
                }),
                environment: None,
            }),
        };

        let outcome: Outcome = run_cli(cli, &self.fs)
            .await
            .context("error calling pin_migration")?;

        match outcome {
            Outcome::PinnedMigration { hash } => Ok(hash),
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
    let helper = MigrationTestHelper::new_empty().await?;

    // Test that we can create a migration
    let migration_name = helper
        .create_migration("test-create")
        .await
        .expect("Failed to create migration with helper");

    let cfg = helper.load_config().await?;

    // Check that <migration folder>/up.sql exists:
    let script_path = format!("{}/up.sql", cfg.pather().migration_folder(&migration_name));

    // Check the contents are what we expect:
    let file_data = helper.fs.read(&script_path).await?.to_bytes();
    let file_contents = String::from_utf8(file_data.to_vec())?;
    assert_eq!(DEFAULT_MIGRATION_CONTENT, file_contents,);

    Ok(())
}

#[tokio::test]
async fn test_migration_build_basic() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_migration_build_with_component() -> Result<(), Box<dyn std::error::Error>> {
    let helper =
        MigrationTestHelper::new_from_local_folder("./static/tests/build_with_component").await?;

    let migration_name = "20240907212659-initial";
    let expected = concat!(
        r#"BEGIN;
-- Created by"#,
        " \n", // prevents trim space from removing the extra space here
        r#"-- Environment: dev


-- uuid var: 9cf58fa3-ed23-5cf3-986c-bb1b76f74b2e

CREATE OR REPLACE FUNCTION add_two_numbers(a NUMERIC, b NUMERIC)
RETURNS NUMERIC AS $$
BEGIN
    RETURN a + b;
END;
$$ LANGUAGE plpgsql;

COMMIT;"#
    );

    // Build the migration
    let built = helper.build_migration(&migration_name, false).await?;
    assert_eq!(expected, built);

    // Pin, and try again:
    let pin_hash = helper.pin_migration(migration_name).await?;
    println!("pinned with hash {}", pin_hash);

    // Now, if we change the contents of util.sql, that should not affect
    // our output.  Replace with new function:
    let new_add_func = concat!(
        r#"CREATE OR REPLACE FUNCTION add_three_numbers(a NUMERIC, b NUMERIC, c NUMERIC)
RETURNS NUMERIC AS $$
BEGIN
    RETURN a + b + c;
END;
$$ LANGUAGE plpgsql;"#
    );

    helper
        .fs
        .write("/db/components/util/add_func.sql", new_add_func)
        .await?;

    // Verify that building migration using pinned components, we get the
    // same expected back.
    let built = helper.build_migration(&migration_name, true).await?;
    assert_eq!(expected, built);

    // But using the unpinned version should use the new function:
    let expected_new = concat!(
        r#"BEGIN;
-- Created by"#,
        " \n",
        r#"-- Environment: dev


-- uuid var: 9cf58fa3-ed23-5cf3-986c-bb1b76f74b2e

CREATE OR REPLACE FUNCTION add_three_numbers(a NUMERIC, b NUMERIC, c NUMERIC)
RETURNS NUMERIC AS $$
BEGIN
    RETURN a + b + c;
END;
$$ LANGUAGE plpgsql;

COMMIT;"#
    );

    let built = helper.build_migration(&migration_name, false).await?;
    assert_eq!(expected_new, built);

    Ok(())
}
