use anyhow::{Context, Result};
use futures::TryStreamExt;
use opendal::services::Memory;
use opendal::Operator;
use pretty_assertions::assert_eq;
use spawn_db::{
    commands::{
        BuildMigration, BuildScript, Check, Command, NewMigration, NewScript, NewTest, Outcome,
        PinCleanup, PinError, PinMigration, PinScript, PinVerify,
    },
    config::{Config, ConfigLoaderSaver},
    engine::{CommandSpec, EngineType, TargetConfig},
    script::PinMode,
    secrets::{SecretDefinition, SecretSource},
    source_config::SourceConfig,
    store,
    variables::Variables,
};
use std::collections::HashMap;
use tokio;

/// Expected default new migration content:
const DEFAULT_MIGRATION_CONTENT: &str = r#"BEGIN;

COMMIT;
"#;

/// Expected default new test content:
const DEFAULT_TEST_CONTENT: &str = "-- Test file\nSELECT 1;\n";

/// Expected default new script content:
const DEFAULT_SCRIPT_CONTENT: &str = r#"BEGIN;

COMMIT;
"#;

/// Reusable test helper structure for setting up migration tests
pub struct MigrationTestHelper {
    pub fs: Operator,
    config_path: String,
}

impl MigrationTestHelper {
    pub fn config_path(&self) -> &str {
        &self.config_path
    }
}

impl MigrationTestHelper {
    pub async fn load_config(&self) -> Result<Config> {
        Config::load(&self.config_path(), &self.fs, None).await
    }

    /// Creates a new test environment with no data
    pub async fn new_empty() -> Result<Self> {
        let mem_service = Memory::default();
        let mem_op = Operator::new(mem_service)?;

        Self::new_from_operator(mem_op).await
    }

    pub async fn new_from_local_folder(folder: &str) -> Result<Self> {
        Self::new_from_operator(Self::operator_from_local_folder(folder).await?).await
    }

    pub async fn operator_from_local_folder(folder: &str) -> Result<Operator> {
        store::disk_to_operator(folder, Some("/db/"), store::DesiredOperator::Memory).await
    }

    pub async fn new_from_operator(op: Operator) -> Result<Self> {
        Self::new_from_operator_with_config(op, Self::default_config_loadersaver()).await
    }

    pub async fn new_from_operator_with_config(
        op: Operator,
        config_loader: ConfigLoaderSaver,
    ) -> Result<Self> {
        let config_path = format!("./{}", spawn_db::config::DEFAULT_CONFIG_FILE);
        let mth = Self {
            fs: op,
            config_path,
        };

        config_loader.save(&mth.config_path, &mth.fs).await?;

        Ok(mth)
    }

    fn default_config_loadersaver() -> ConfigLoaderSaver {
        let mut targets = HashMap::new();
        targets.insert(
            "postgres_psql".to_string(),
            TargetConfig {
                engine: EngineType::PostgresPSQL,
                spawn_database: Some("spawn".to_string()),
                spawn_schema: "public".to_string(),
                environment: "dev".to_string(),
                command: Some(CommandSpec::Direct {
                    direct: vec![
                        "docker".to_string(),
                        "exec".to_string(),
                        "-i".to_string(),
                        "spawn-db".to_string(),
                        "psql".to_string(),
                        "-U".to_string(),
                        "spawn".to_string(),
                        "spawn".to_string(),
                    ],
                }),
            },
        );

        ConfigLoaderSaver {
            spawn_folder: "/db".to_string(),
            target: Some("postgres_psql".to_string()),
            environment: Some("dev".to_string()),
            template_up: None,
            template_test: None,
            template_script: None,
            targets: Some(targets),
            secrets: None,
            project_id: None,
            telemetry: Some(false),
        }
    }

    /// Creates a new migration using the NewMigration command
    pub async fn create_migration(&self, name: &str) -> Result<String, anyhow::Error> {
        let config = self.load_config().await?;
        let cmd = NewMigration {
            name: name.to_string(),
        };

        let outcome = cmd.execute(&config).await?;

        match outcome {
            Outcome::NewMigration(name) => Ok(name),
            _ => Err(anyhow::anyhow!(
                "Migration directory not found after creation"
            )),
        }
    }

    /// Creates a new test using the NewTest command
    pub async fn create_test(&self, name: &str) -> Result<String, anyhow::Error> {
        let config = self.load_config().await?;
        let cmd = NewTest {
            name: name.to_string(),
        };

        let outcome = cmd.execute(&config).await?;

        match outcome {
            Outcome::NewTest(name) => Ok(name),
            _ => Err(anyhow::anyhow!("Test directory not found after creation")),
        }
    }

    /// Creates a new script using the NewScript command
    pub async fn create_script(&self, script: &str) -> Result<String, anyhow::Error> {
        let config = self.load_config().await?;
        let cmd = NewScript {
            script: script.to_string(),
        };

        let outcome = cmd.execute(&config).await?;

        match outcome {
            Outcome::NewScript { script, .. } => Ok(script),
            _ => Err(anyhow::anyhow!("Script directory not found after creation")),
        }
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
        let migration_path = cfg.pather().migration_script_file_path(migration_name);
        self.fs.write(&migration_path, script_content).await?;

        Ok(migration_name.clone())
    }

    /// Builds a migration using the BuildMigration command
    pub async fn build_migration(
        &self,
        migration_name: &str,
        pinned: bool,
    ) -> Result<String, anyhow::Error> {
        self.build_migration_with_variables(migration_name, pinned, None)
            .await
    }

    /// Builds a migration with optional variables file
    pub async fn build_migration_with_variables(
        &self,
        migration_name: &str,
        pinned: bool,
        variables_path: Option<String>,
    ) -> Result<String, anyhow::Error> {
        self.build_migration_with_variable_overrides(migration_name, pinned, variables_path, &[])
            .await
    }

    /// Builds a migration with an optional variables file and `--var`
    /// overrides layered on top, exactly as the CLI resolves them.
    pub async fn build_migration_with_variable_overrides(
        &self,
        migration_name: &str,
        pinned: bool,
        variables_path: Option<String>,
        overrides: &[(String, serde_json::Value)],
    ) -> Result<String, anyhow::Error> {
        let config = self.load_config().await?;

        let base = match variables_path {
            Some(path) => Some(config.load_variables_from_path(&path).await?),
            None => None,
        };
        let variables = Variables::with_overrides(base, overrides)?;

        let cmd = BuildMigration {
            migration: migration_name.to_string(),
            pinned,
            variables,
            reveal_secrets: false,
            use_defaults: false,
        };

        let outcome = cmd.execute(&config).await?;

        match outcome {
            Outcome::BuiltMigration {
                content,
                pinned_warn: _pinned_warn,
            } => Ok(content),
            _ => Err(anyhow::anyhow!("Unexpected outcome")),
        }
    }

    /// Builds a migration taking each declared `default` rather than asking,
    /// so a test can never block on a prompt when run from a terminal.
    pub async fn build_migration_with_defaults(
        &self,
        migration_name: &str,
        pinned: bool,
    ) -> Result<String, anyhow::Error> {
        let config = self.load_config().await?;
        let cmd = BuildMigration {
            migration: migration_name.to_string(),
            pinned,
            variables: None,
            reveal_secrets: false,
            use_defaults: true,
        };

        match cmd.execute(&config).await? {
            Outcome::BuiltMigration { content, .. } => Ok(content),
            _ => Err(anyhow::anyhow!("Unexpected outcome")),
        }
    }

    /// Writes a migration's `lock.toml` at `name` (relative to `migrations/`).
    pub async fn write_migration_lock(&self, name: &str, contents: &str) -> Result<()> {
        let cfg = self.load_config().await?;
        let path = cfg.pather().migration_lock_file_path(name);
        self.fs.write(&path, contents.to_string()).await?;
        Ok(())
    }

    /// Reads a migration's `lock.toml` back as text.
    pub async fn read_migration_lock(&self, name: &str) -> Result<String> {
        let cfg = self.load_config().await?;
        let path = cfg.pather().migration_lock_file_path(name);
        let bytes = self.fs.read(&path).await?.to_bytes();
        Ok(String::from_utf8(bytes.to_vec())?)
    }

    /// Pins a migration using the PinMigration command
    pub async fn pin_migration(&self, migration_name: &str) -> Result<String, anyhow::Error> {
        let config = self.load_config().await?;
        let cmd = PinMigration {
            migration: migration_name.to_string(),
        };

        let outcome = cmd
            .execute(&config)
            .await
            .context("error calling pin_migration")?;

        match outcome {
            Outcome::PinnedMigration { hash } => Ok(hash),
            _ => Err(anyhow::anyhow!("Unexpected outcome")),
        }
    }

    /// Writes a migration's up.sql at `name` (relative to `migrations/`).
    pub async fn write_migration(&self, name: &str, contents: &str) -> Result<()> {
        let cfg = self.load_config().await?;
        let path = cfg.pather().migration_script_file_path(name);
        self.fs.write(&path, contents.to_string()).await?;
        Ok(())
    }

    /// Writes a component at `name` (relative to the components folder).
    pub async fn write_component(&self, name: &str, contents: &str) -> Result<()> {
        let cfg = self.load_config().await?;
        let path = format!("{}/{}", cfg.pather().components_folder(), name);
        self.fs.write(&path, contents.to_string()).await?;
        Ok(())
    }

    /// Writes a script's entry template at `name` (relative to `scripts/`).
    pub async fn write_script(&self, name: &str, contents: &str) -> Result<()> {
        let cfg = self.load_config().await?;
        let path = cfg.pather().script_file_path(name);
        self.fs.write(&path, contents.to_string()).await?;
        Ok(())
    }

    /// Builds a script using the BuildScript command.
    pub async fn build_script(
        &self,
        script: &str,
        pin_mode: PinMode,
        variables: Option<Variables>,
    ) -> Result<String, anyhow::Error> {
        let config = self.load_config().await?;
        let cmd = BuildScript {
            script: script.to_string(),
            pin_mode,
            variables,
            reveal_secrets: false,
            use_defaults: false,
        };

        match cmd.execute(&config).await? {
            Outcome::BuiltScript { content } => Ok(content),
            _ => Err(anyhow::anyhow!("Unexpected outcome")),
        }
    }

    /// Pins a script using the PinScript command.
    pub async fn pin_script(&self, script: &str) -> Result<String, anyhow::Error> {
        let config = self.load_config().await?;
        let cmd = PinScript {
            script: script.to_string(),
        };

        match cmd
            .execute(&config)
            .await
            .context("error calling pin_script")?
        {
            Outcome::PinnedScript { hash, .. } => Ok(hash),
            _ => Err(anyhow::anyhow!("Unexpected outcome")),
        }
    }

    pub async fn _list_fs_contents(&self, label: &str) -> Result<()> {
        let mut lister = self.fs.lister_with(".").recursive(true).await?;

        println!("listing files for '{}'", label);
        while let Some(entry) = lister.try_next().await? {
            let file_data = self.fs.read(&entry.path()).await?.to_bytes();
            println!("(len {}). found {}", file_data.len(), entry.path());
        }

        Ok(())
    }

    /// Collect all files and their contents as a map for comparison
    pub async fn collect_all_files(&self) -> Result<HashMap<String, Vec<u8>>> {
        let mut lister = self.fs.lister_with("/").recursive(true).await?;
        let mut files = HashMap::new();

        while let Some(entry) = lister.try_next().await? {
            // Skip directories
            if entry.path().ends_with('/') {
                continue;
            }
            let file_data = self.fs.read(entry.path()).await?.to_bytes().to_vec();
            files.insert(entry.path().to_string(), file_data);
        }

        Ok(files)
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

// Run a create migration test using a custom template_up:
#[tokio::test]
async fn test_create_migration_with_custom_template() -> Result<(), Box<dyn std::error::Error>> {
    const CUSTOM_TEMPLATE_CONTENT: &str = "-- custom migration template\nBEGIN;\n\nCOMMIT;\n";

    let mem_op = Operator::new(Memory::default())?;
    let mut config_loader = MigrationTestHelper::default_config_loadersaver();
    config_loader.template_up = Some("templates/custom.sql".to_string());
    let helper = MigrationTestHelper::new_from_operator_with_config(mem_op, config_loader).await?;

    let cfg = helper.load_config().await?;
    let template_path = cfg.pather().any_path("templates/custom.sql");
    helper
        .fs
        .write(&template_path, CUSTOM_TEMPLATE_CONTENT)
        .await?;

    // Test that we can create a migration
    let migration_name = helper
        .create_migration("test-create")
        .await
        .expect("Failed to create migration with helper");

    // Check the contents are what we expect:
    let script_path = format!("{}/up.sql", cfg.pather().migration_folder(&migration_name));
    let file_data = helper.fs.read(&script_path).await?.to_bytes();
    let file_contents = String::from_utf8(file_data.to_vec())?;
    assert_eq!(CUSTOM_TEMPLATE_CONTENT, file_contents,);

    Ok(())
}

// Run a create test test:
#[tokio::test]
async fn test_create_test() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;

    // Test that we can create a test
    let test_name = helper
        .create_test("test-create")
        .await
        .expect("Failed to create test with helper");

    let cfg = helper.load_config().await?;

    // Check that <test folder>/test.sql exists and has the default content:
    let script_path = cfg.pather().test_file_path(&test_name);
    let file_data = helper.fs.read(&script_path).await?.to_bytes();
    let file_contents = String::from_utf8(file_data.to_vec())?;
    assert_eq!(DEFAULT_TEST_CONTENT, file_contents,);

    Ok(())
}

// Run a create test test using a custom template_test:
#[tokio::test]
async fn test_create_test_with_custom_template() -> Result<(), Box<dyn std::error::Error>> {
    const CUSTOM_TEMPLATE_CONTENT: &str = "-- custom test template\nSELECT 2;\n";

    let mem_op = Operator::new(Memory::default())?;
    let mut config_loader = MigrationTestHelper::default_config_loadersaver();
    config_loader.template_test = Some("templates/custom.sql".to_string());
    let helper = MigrationTestHelper::new_from_operator_with_config(mem_op, config_loader).await?;

    let cfg = helper.load_config().await?;
    let template_path = cfg.pather().any_path("templates/custom.sql");
    helper
        .fs
        .write(&template_path, CUSTOM_TEMPLATE_CONTENT)
        .await?;

    // Test that we can create a test
    let test_name = helper
        .create_test("test-create")
        .await
        .expect("Failed to create test with helper");

    // Check the contents are what we expect:
    let script_path = cfg.pather().test_file_path(&test_name);
    let file_data = helper.fs.read(&script_path).await?.to_bytes();
    let file_contents = String::from_utf8(file_data.to_vec())?;
    assert_eq!(CUSTOM_TEMPLATE_CONTENT, file_contents,);

    Ok(())
}

// Run a create script test, over the name shapes a project may reasonably use.
// Only a segment that is exactly '.' or '..' is rejected, so dots within a
// segment stay legal; the counterpart negative cases are in the test below.
#[tokio::test]
async fn test_create_script() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;
    let cfg = helper.load_config().await?;

    for name in [
        "reindex",
        "maintenance/reindex",
        "maintenance/weekly/vacuum-full",
        "v1.2-cleanup",
        "backfill.step1",
        "..hidden",
        "trailing.",
    ] {
        let script = helper
            .create_script(name)
            .await
            .unwrap_or_else(|e| panic!("script name '{}' should be accepted: {}", name, e));
        assert_eq!(name, script);

        // The script lands in its own folder under scripts/, which is what lets
        // the commands that walk scripts/ find it again.
        let script_path = cfg.pather().script_file_path(&script);
        assert!(
            script_path.ends_with(&format!("/scripts/{}/script.sql", name)),
            "unexpected path for '{}': {}",
            name,
            script_path
        );

        let file_data = helper.fs.read(&script_path).await?.to_bytes();
        let file_contents = String::from_utf8(file_data.to_vec())?;
        assert_eq!(DEFAULT_SCRIPT_CONTENT, file_contents);
    }

    Ok(())
}

#[tokio::test]
async fn test_create_script_rejects_names_outside_its_own_folder(
) -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;

    for name in ["", ".", "..", "/abs", "a//b", "a/../b", "nested/."] {
        let err = helper
            .create_script(name)
            .await
            .expect_err(&format!("script name '{}' should be rejected", name));
        assert!(
            err.to_string().contains("invalid script name"),
            "unexpected error for '{}': {}",
            name,
            err
        );
    }

    Ok(())
}

// Run a create script test using a custom template_script:
#[tokio::test]
async fn test_create_script_with_custom_template() -> Result<(), Box<dyn std::error::Error>> {
    const CUSTOM_TEMPLATE_CONTENT: &str = "-- custom script template\nVACUUM;\n";

    let mem_op = Operator::new(Memory::default())?;
    let mut config_loader = MigrationTestHelper::default_config_loadersaver();
    config_loader.template_script = Some("templates/custom.sql".to_string());
    let helper = MigrationTestHelper::new_from_operator_with_config(mem_op, config_loader).await?;

    let cfg = helper.load_config().await?;
    let template_path = cfg.pather().any_path("templates/custom.sql");
    helper
        .fs
        .write(&template_path, CUSTOM_TEMPLATE_CONTENT)
        .await?;

    // Test that we can create a script
    let script = helper
        .create_script("maintenance/reindex")
        .await
        .expect("Failed to create script with helper");

    // Check the contents are what we expect:
    let script_path = cfg.pather().script_file_path(&script);
    let file_data = helper.fs.read(&script_path).await?.to_bytes();
    let file_contents = String::from_utf8(file_data.to_vec())?;
    assert_eq!(CUSTOM_TEMPLATE_CONTENT, file_contents,);

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
-- Created by unknown
-- Environment: dev


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
-- Created by unknown
-- Environment: dev


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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_build_with_pinned_fails_if_pinned_file_tampered_with(
) -> Result<(), Box<dyn std::error::Error>> {
    let helper =
        MigrationTestHelper::new_from_local_folder("./static/tests/build_with_component").await?;

    let migration_name = "20240907212659-initial";
    helper.pin_migration(migration_name).await?;

    // Corrupt the pinned blob directly on disk (bypassing pin/build), the
    // same way a stray edit to a file under pinned/ would. Its path is
    // content-addressed by the *original* content's hash, so this simulates
    // the path and the bytes at that path disagreeing.
    let original_component = helper
        .fs
        .read("/db/components/util/add_func.sql")
        .await?
        .to_bytes();
    let hash = store::pinner::pin_contents(&helper.fs, None, &original_component).await?;
    let pinned_path = format!("/db/pinned/{}", store::pinner::hash_to_path(&hash)?);
    helper
        .fs
        .write(&pinned_path, "-- tampered contents".to_string())
        .await?;

    let result = helper.build_migration(migration_name, true).await;
    let err = result.expect_err("build with a tampered pinned file should fail");
    let err_msg = format!("{:?}", err);
    assert!(
        err_msg.contains("does not match its expected hash"),
        "expected a hash-mismatch error, got: {}",
        err_msg
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_migration_build_with_variables() -> Result<(), Box<dyn std::error::Error>> {
    let helper =
        MigrationTestHelper::new_from_local_folder("./static/tests/build_with_variables").await?;

    let migration_name = "20240907212659-initial";

    // Must fail rather than silently render an empty identifier and VARCHAR length.
    let err = helper
        .build_migration(&migration_name, false)
        .await
        .expect_err("a template referencing unset variables should not build");
    let err_msg = format!("{:?}", err);
    assert!(
        err_msg.contains("undefined value"),
        "expected an undefined-value error, got: {}",
        err_msg
    );

    // Build with variables file
    let built_with_vars = helper
        .build_migration_with_variables(
            migration_name,
            false,
            Some("/db/variables.json".to_string()),
        )
        .await?;

    let expected = r#"BEGIN;
-- Migration: create-users-table
-- Author: Test Author
-- Environment: dev

CREATE TABLE "users" (
    id SERIAL PRIMARY KEY,
    name VARCHAR(255) NOT NULL,
    active BOOLEAN DEFAULT TRUE
);

COMMIT;"#;

    assert_eq!(expected, built_with_vars);

    Ok(())
}

/// Every type `--var-json` accepts, rendered. The number/string and
/// boolean/string pairs are the distinction the flag exists to make, and the
/// boolean pair is the one that changes which branch runs rather than only how
/// the value is escaped.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_var_json_overrides_reach_sql_with_their_json_type(
) -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;

    let template = r#"num:        {{ variables.num }}
num_str:    {{ variables.num_str }}
flag:       {{ variables.flag }}
flag_str:   {{ variables.flag_str }}
branch:     {% if variables.flag %}TRUE-BRANCH{% else %}FALSE-BRANCH{% endif %}
branch_str: {% if variables.flag_str %}TRUE-BRANCH{% else %}FALSE-BRANCH{% endif %}
tags:       {{ variables.tags }}
nested:     {{ variables.limits.max }}
bare:       VARCHAR({{ variables.num }})"#;

    let migration = helper
        .create_migration_manual("var-json-types", template.to_string())
        .await?;

    let built = helper
        .build_migration_with_variable_overrides(
            &migration,
            false,
            None,
            &[
                ("num".to_string(), serde_json::json!(42)),
                ("num_str".to_string(), serde_json::json!("42")),
                ("flag".to_string(), serde_json::json!(false)),
                ("flag_str".to_string(), serde_json::json!("false")),
                ("tags".to_string(), serde_json::json!(["a", "b"])),
                ("limits".to_string(), serde_json::json!({"max": 10})),
            ],
        )
        .await?;

    let expected = r#"num:        42
num_str:    '42'
flag:       FALSE
flag_str:   'false'
branch:     FALSE-BRANCH
branch_str: TRUE-BRANCH
tags:       ARRAY['a', 'b']
nested:     10
bare:       VARCHAR(42)"#;

    assert_eq!(expected, built);

    Ok(())
}

/// `--var-json` layers over a `--variables` file for migrations, the same way
/// it does for scripts: the override wins, and the keys it does not name keep
/// the types the file gave them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_migration_build_with_var_overrides() -> Result<(), Box<dyn std::error::Error>> {
    let helper =
        MigrationTestHelper::new_from_local_folder("./static/tests/build_with_variables").await?;

    let built = helper
        .build_migration_with_variable_overrides(
            "20240907212659-initial",
            false,
            Some("/db/variables.json".to_string()),
            &[
                ("table_name".to_string(), serde_json::json!("customers")),
                ("author".to_string(), serde_json::json!("Override Author")),
            ],
        )
        .await?;

    // The override replaces the file's value for the keys it names...
    assert!(
        built.contains(r#"CREATE TABLE "customers""#),
        "got: {}",
        built
    );
    assert!(
        built.contains("-- Author: Override Author"),
        "got: {}",
        built
    );
    // ...and leaves the rest of the file alone, typed as the file had them.
    assert!(
        built.contains("-- Migration: create-users-table"),
        "got: {}",
        built
    );
    assert!(built.contains("VARCHAR(255)"), "got: {}", built);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_migration_build_with_read_file_and_parse() -> Result<(), Box<dyn std::error::Error>> {
    let helper =
        MigrationTestHelper::new_from_local_folder("./static/tests/build_with_read_file").await?;

    let migration_name = "20240907212659-initial";
    let built = helper.build_migration(migration_name, false).await?;

    let expected = r#"BEGIN;

-- from json
CREATE TABLE "users" (id SERIAL PRIMARY KEY);
SELECT * FROM "users" LIMIT 100;

-- from toml
SELECT * FROM "users" LIMIT 100;

-- from yaml
SELECT * FROM "users" LIMIT 100;

-- from json array
INSERT INTO "users" (name, email) VALUES ('alice', 'alice@example.com');
INSERT INTO "users" (name, email) VALUES ('bob', 'bob@example.com');

COMMIT;"#;

    assert_eq!(expected, built);

    Ok(())
}

#[tokio::test]
async fn test_check_passes_with_no_migrations() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;
    let config = helper.load_config().await?;

    let outcome = Check.execute(&config).await?;
    assert!(matches!(outcome, Outcome::Success));

    Ok(())
}

#[tokio::test]
async fn test_check_fails_with_unpinned_migration() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;
    helper.create_migration("unpinned-migration").await?;

    let config = helper.load_config().await?;
    let outcome = Check.execute(&config).await?;
    assert!(matches!(outcome, Outcome::CheckFailed));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_check_passes_when_all_pinned() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;
    let migration_name = helper.create_migration("will-be-pinned").await?;
    helper.pin_migration(&migration_name).await?;

    let config = helper.load_config().await?;
    let outcome = Check.execute(&config).await?;
    assert!(matches!(outcome, Outcome::Success));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_build_reports_an_unknown_migration_as_missing(
) -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;
    helper.create_migration("real-migration").await?;

    // Both modes, since neither a typo nor a real migration has a lock.toml
    // until it is pinned.
    for pinned in [true, false] {
        let err = helper
            .build_migration("20240101000000-typo", pinned)
            .await
            .expect_err("an unknown migration should not build");
        let message = format!("{:#}", err);
        assert!(
            message.contains("not found"),
            "pinned={}: {}",
            pinned,
            message
        );
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_pin_preserves_a_hand_written_lock_toml() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;
    let migration_name = helper.create_migration("hand-edited").await?;
    let config = helper.load_config().await?;
    let lock_path = config.pather().migration_lock_file_path(&migration_name);

    let authored = r#"# why this migration declares anything at all
future_key = "spawn does not recognise this one"

[variables.tenant]
source = "prompt"

# alphabetically earlier, deliberately written second
[variables.admin_email]
source = "value"
value = "admin@example.com"
"#;
    helper.fs.write(&lock_path, authored).await?;

    helper.pin_migration(&migration_name).await?;

    let written = String::from_utf8(helper.fs.read(&lock_path).await?.to_bytes().to_vec())?;
    for kept in [
        "# why this migration declares anything at all",
        "# alphabetically earlier, deliberately written second",
        "future_key = \"spawn does not recognise this one\"",
    ] {
        assert!(written.contains(kept), "lost {:?} from:\n{}", kept, written);
    }
    assert!(
        written.find("[variables.tenant]") < written.find("[variables.admin_email]"),
        "declarations were reordered:\n{}",
        written
    );
    // The new key has to join the root table; appended after the first
    // [variables.*] header it would silently become a variable instead.
    assert!(
        written.find("pin = ") < written.find('['),
        "pin landed inside a table:\n{}",
        written
    );

    // Still readable as a pinned config afterwards.
    let reloaded: SourceConfig = config.load_source_config(&lock_path).await?;
    assert!(reloaded.pin.is_some());
    assert_eq!(2, reloaded.variables.len());

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_check_reports_a_malformed_lock_toml() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;
    let migration_name = helper.create_migration("will-be-corrupted").await?;
    helper.pin_migration(&migration_name).await?;

    let config = helper.load_config().await?;
    helper
        .fs
        .write(
            &config.pather().migration_lock_file_path(&migration_name),
            "pin = [",
        )
        .await?;

    // A warning rather than a hard error, so the rest of the check still runs;
    // and not silence either, since this migration was passing before.
    let outcome = Check.execute(&config).await?;
    assert!(matches!(outcome, Outcome::CheckFailed));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_pin_fails_when_migration_does_not_exist() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;

    // Add a component so that pinning would create files if it ran
    let config = helper.load_config().await?;
    helper
        .fs
        .write(
            &format!("{}/example.sql", config.pather().components_folder()),
            "SELECT 1;",
        )
        .await?;

    // Capture files before attempting to pin
    let files_before = helper.collect_all_files().await?;
    let component_path = format!("{}/example.sql", config.pather().components_folder());
    // Strip leading slash since collect_all_files returns paths without it
    let component_path = component_path.trim_start_matches('/');
    assert!(
        files_before.contains_key(component_path),
        "Component file should exist at {}",
        component_path
    );

    // Try to pin a migration that doesn't exist
    let cmd = PinMigration {
        migration: "nonexistent-migration".to_string(),
    };

    let Err(err) = cmd.execute(&config).await else {
        panic!("Pinning a non-existent migration should fail");
    };

    // Check that the error is the specific PinError::MigrationNotFound type
    let pin_error = err
        .downcast_ref::<PinError>()
        .expect("Error should be a PinError");
    assert!(
        matches!(pin_error, PinError::MigrationNotFound(_)),
        "Expected PinError::MigrationNotFound, got: {:?}",
        pin_error
    );

    // Verify no files were created or modified
    let files_after = helper.collect_all_files().await?;
    assert_eq!(
        files_before, files_after,
        "No files should be created or modified when pinning fails"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_build_and_pin_with_binary_file() -> Result<(), Box<dyn std::error::Error>> {
    // This test verifies that pinning works with binary files in components/
    let helper =
        MigrationTestHelper::new_from_local_folder("./static/tests/build_and_pin_pdf").await?;

    // Verify the binary file actually contains invalid UTF-8 (this would have failed with old code)
    let binary_bytes = helper.fs.read("/db/components/small.bin").await?.to_bytes();
    assert!(
        String::from_utf8(binary_bytes.to_vec()).is_err(),
        "small.bin should contain invalid UTF-8 bytes"
    );

    let migration_name = "20240907212659-initial";

    // Build the migration - it reads a binary file and base64 encodes it
    let built = helper.build_migration(&migration_name, false).await?;

    // small.bin contains bytes [0x00, 0x01, 0x02, 0xff, 0xfe, 0xfd] which base64 encodes to "AAEC//79"
    // The quotes are escaped/doubled by the SQL auto-escape
    let expected = r#"BEGIN;

SELECT 'AAEC//79';

COMMIT;"#;
    assert_eq!(expected, built);

    // Pin the migration - this should work even with a binary file in components/
    let pin_hash = helper.pin_migration(migration_name).await?;
    assert!(!pin_hash.is_empty(), "Pin hash should not be empty");

    // Build with pinned=true to verify pinned content works
    let built_pinned = helper.build_migration(&migration_name, true).await?;
    assert_eq!(expected, built_pinned);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_build_shows_warning_when_pinned_but_not_using_pinned_flag(
) -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;

    let script_content = r#"BEGIN;
CREATE TABLE test (id SERIAL PRIMARY KEY);
COMMIT;"#;

    let migration_name = helper
        .create_migration_manual("test-pinned-warning", script_content.to_string())
        .await?;

    // Pin the migration
    helper.pin_migration(&migration_name).await?;

    let config = helper.load_config().await?;

    // Build without --pinned flag - should still succeed but show warning
    let cmd = BuildMigration {
        migration: migration_name.to_string(),
        pinned: false,
        variables: None,
        reveal_secrets: false,
        use_defaults: false,
    };

    let outcome = cmd.execute(&config).await?;
    assert!(matches!(outcome, Outcome::BuiltMigration { .. }));

    // Build with --pinned flag - should not show warning
    let cmd_pinned = BuildMigration {
        migration: migration_name.to_string(),
        pinned: true,
        variables: None,
        reveal_secrets: false,
        use_defaults: false,
    };

    let outcome_pinned = cmd_pinned.execute(&config).await?;
    assert!(matches!(outcome_pinned, Outcome::BuiltMigration { .. }));

    Ok(())
}

#[tokio::test]
async fn test_pin_cleanup_no_orphans() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;

    // Create and pin a migration
    let migration_name = helper.create_migration("test-migration").await?;
    helper.pin_migration(&migration_name).await?;

    let config = helper.load_config().await?;
    let files_before = helper.collect_all_files().await?;

    // Run cleanup - should find no orphans and change nothing
    let cmd = PinCleanup { dry_run: false };
    let outcome = cmd.execute(&config).await?;

    let Outcome::PinCleanup {
        orphaned,
        referenced_count,
        dry_run,
    } = outcome
    else {
        panic!("Expected Outcome::PinCleanup");
    };

    assert!(orphaned.is_empty(), "should have no orphans");
    assert!(referenced_count > 0, "should have referenced files");
    assert!(!dry_run);

    let files_after = helper.collect_all_files().await?;
    assert_eq!(
        files_before, files_after,
        "cleanup should not change any files when there are no orphans"
    );

    Ok(())
}

#[tokio::test]
async fn test_pin_cleanup_finds_and_deletes_orphans() -> Result<(), Box<dyn std::error::Error>> {
    let helper =
        MigrationTestHelper::new_from_local_folder("./static/tests/gc_clean_unreffed_pin").await?;

    let config = helper.load_config().await?;

    let files_before = helper.collect_all_files().await?;

    // Run cleanup dry-run - should report orphans but not delete anything
    let cmd = PinCleanup { dry_run: true };
    let outcome = cmd.execute(&config).await?;

    let Outcome::PinCleanup {
        orphaned,
        referenced_count,
        dry_run,
    } = outcome
    else {
        panic!("Expected Outcome::PinCleanup");
    };

    assert_eq!(orphaned.len(), 4, "dry-run should report 4 orphans");
    assert_eq!(referenced_count, 2, "should have 2 referenced files");
    assert!(dry_run);

    let files_after_dry_run = helper.collect_all_files().await?;
    assert_eq!(
        files_before, files_after_dry_run,
        "dry-run should not change any files"
    );

    // Run cleanup for real - should delete orphaned pinned files
    let cmd = PinCleanup { dry_run: false };
    let outcome = cmd.execute(&config).await?;

    let Outcome::PinCleanup {
        orphaned,
        referenced_count,
        dry_run,
    } = outcome
    else {
        panic!("Expected Outcome::PinCleanup");
    };

    assert_eq!(orphaned.len(), 4, "should delete 4 orphans");
    assert_eq!(referenced_count, 2, "should have 2 referenced files");
    assert!(!dry_run);

    let files_after_cleanup = helper.collect_all_files().await?;
    assert_eq!(
        files_before.len() - 4,
        files_after_cleanup.len(),
        "cleanup should delete 4 orphaned pinned files"
    );

    Ok(())
}

#[tokio::test]
async fn test_pin_verify_passes_on_untampered_store() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;

    let migration_name = helper.create_migration("test-migration").await?;
    helper.pin_migration(&migration_name).await?;

    let config = helper.load_config().await?;
    let outcome = PinVerify.execute(&config).await?;

    let Outcome::PinVerify {
        corrupted_count,
        missing_root_count,
    } = outcome
    else {
        panic!("Expected Outcome::PinVerify");
    };

    assert_eq!(corrupted_count, 0);
    assert_eq!(missing_root_count, 0);

    Ok(())
}

#[tokio::test]
async fn test_pin_verify_detects_corrupted_file() -> Result<(), Box<dyn std::error::Error>> {
    let helper =
        MigrationTestHelper::new_from_local_folder("./static/tests/build_with_component").await?;

    let migration_name = "20240907212659-initial";
    helper.pin_migration(migration_name).await?;

    // Tamper with a pinned blob directly on disk, the same way a stray edit
    // to a file under pinned/ would: its path stays content-addressed by the
    // *original* content's hash, so the path and the bytes at that path now
    // disagree.
    let original_component = helper
        .fs
        .read("/db/components/util/add_func.sql")
        .await?
        .to_bytes();
    let hash = store::pinner::pin_contents(&helper.fs, None, &original_component).await?;
    let pinned_path = format!("/db/pinned/{}", store::pinner::hash_to_path(&hash)?);
    helper
        .fs
        .write(&pinned_path, "-- tampered contents".to_string())
        .await?;

    let config = helper.load_config().await?;
    let outcome = PinVerify.execute(&config).await?;

    let Outcome::PinVerify {
        corrupted_count,
        missing_root_count,
    } = outcome
    else {
        panic!("Expected Outcome::PinVerify");
    };

    assert_eq!(corrupted_count, 1, "should detect the one tampered file");
    assert_eq!(missing_root_count, 0);

    Ok(())
}

#[tokio::test]
async fn test_pin_verify_detects_missing_root() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;

    let migration_name = helper.create_migration("test-migration").await?;
    helper.pin_migration(&migration_name).await?;

    // Point the migration's lock.toml at a root hash that doesn't exist in
    // the store, simulating a pinned file (or its whole subtree) having been
    // deleted out from under a migration that still references it.
    let lock_path = format!("/db/migrations/{}/lock.toml", migration_name);
    helper
        .fs
        .write(
            &lock_path,
            "pin = \"00000000000000000000000000000000\"".to_string(),
        )
        .await?;

    let config = helper.load_config().await?;
    let outcome = PinVerify.execute(&config).await?;

    let Outcome::PinVerify {
        corrupted_count,
        missing_root_count,
    } = outcome
    else {
        panic!("Expected Outcome::PinVerify");
    };

    assert_eq!(corrupted_count, 0);
    assert_eq!(missing_root_count, 1, "should detect the missing root hash");

    Ok(())
}

/// The cause, and the component and migration lines it came from, must all
/// survive minijinja's "could not render include" wrapper.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_migration_build_undefined_secret_in_component_names_cause_and_location(
) -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;
    let config = helper.load_config().await?;

    helper
        .fs
        .write(
            &format!("{}/roles/app_role.sql", config.pather().components_folder()),
            r#"CREATE ROLE app_user WITH LOGIN PASSWORD {{ secret("application_password") }};"#,
        )
        .await?;

    let migration_name = helper
        .create_migration_manual(
            "create-app-role",
            "BEGIN;\n\n{% include \"roles/app_role.sql\" %}\n\nCOMMIT;\n".to_string(),
        )
        .await?;

    let err = helper
        .build_migration(&migration_name, false)
        .await
        .expect_err("a migration referencing an undefined secret must fail to build");
    let chain = format!("{:?}", err);

    assert!(
        chain.contains("secret 'application_password' is not defined in spawn.toml"),
        "the real cause must survive minijinja's include wrapper, got: {chain}"
    );
    assert!(
        chain.contains("none are defined"),
        "with no secrets configured at all, the message should say so, got: {chain}"
    );
    assert!(
        chain.contains("roles/app_role.sql:1"),
        "the cause must name the component and the line within it, got: {chain}"
    );
    assert!(
        chain.contains(&format!(
            "{}:3",
            config.pather().migration_script_file_path(&migration_name)
        )),
        "the outer frame must name the migration's real path and line, got: {chain}"
    );
    assert!(
        !chain.contains("(in migration.sql"),
        "the hardcoded placeholder template name must be gone, got: {chain}"
    );

    Ok(())
}

/// Exercises the full stack for secrets: a [secrets.*] table round-tripped
/// through real TOML (via ConfigLoaderSaver::save/Config::load), an
/// environment-specific override, and both masked and revealed builds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_migration_build_with_secrets() -> Result<(), Box<dyn std::error::Error>> {
    let mut targets = HashMap::new();
    targets.insert(
        "postgres_psql".to_string(),
        TargetConfig {
            engine: EngineType::PostgresPSQL,
            spawn_database: Some("spawn".to_string()),
            spawn_schema: "public".to_string(),
            environment: "dev".to_string(),
            command: None,
        },
    );

    let mut secrets = HashMap::new();
    secrets.insert(
        "application_password".to_string(),
        SecretDefinition {
            default: SecretSource::Literal {
                value: "prod-secret".to_string(),
                insecure: true,
            },
            environments: HashMap::from([(
                "dev".to_string(),
                SecretSource::Literal {
                    value: "dev-secret".to_string(),
                    insecure: true,
                },
            )]),
        },
    );

    let config_loader = ConfigLoaderSaver {
        spawn_folder: "/db".to_string(),
        target: Some("postgres_psql".to_string()),
        environment: Some("dev".to_string()),
        template_up: None,
        template_test: None,
        template_script: None,
        targets: Some(targets),
        secrets: Some(secrets),
        project_id: None,
        telemetry: Some(false),
    };

    let mem_op = Operator::new(Memory::default())?;
    let helper = MigrationTestHelper::new_from_operator_with_config(mem_op, config_loader).await?;

    let migration_name = helper
        .create_migration_manual(
            "with-secret",
            "SELECT {{ secret(\"application_password\") }};".to_string(),
        )
        .await?;

    let config = helper.load_config().await?;

    // Masked build (default): must not leak either the default or the
    // environment-specific value.
    let masked_outcome = BuildMigration {
        migration: migration_name.clone(),
        pinned: false,
        variables: None,
        reveal_secrets: false,
        use_defaults: false,
    }
    .execute(&config)
    .await?;
    let masked_content = match masked_outcome {
        Outcome::BuiltMigration { content, .. } => content,
        _ => panic!("unexpected outcome"),
    };
    assert!(masked_content.contains("***MASKED:application_password***"));
    assert!(!masked_content.contains("dev-secret"));
    assert!(!masked_content.contains("prod-secret"));

    // Revealed build: current environment is "dev", so the override wins
    // over the default.
    let revealed_outcome = BuildMigration {
        migration: migration_name,
        pinned: false,
        variables: None,
        reveal_secrets: true,
        use_defaults: false,
    }
    .execute(&config)
    .await?;
    let revealed_content = match revealed_outcome {
        Outcome::BuiltMigration { content, .. } => content,
        _ => panic!("unexpected outcome"),
    };
    assert!(revealed_content.contains("dev-secret"));
    assert!(!revealed_content.contains("prod-secret"));

    Ok(())
}

/// A script renders like a migration: it includes components, and its
/// variables are escaped the same way.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_script_build_renders_includes_and_variables() -> Result<(), Box<dyn std::error::Error>>
{
    let helper = MigrationTestHelper::new_empty().await?;

    helper
        .write_component("util/touch.sql", "SELECT touch_tenant();")
        .await?;
    helper
        .write_script(
            "maintenance/reindex",
            "-- tenant {{ variables.tenant }}\n{% include \"util/touch.sql\" %}\n",
        )
        .await?;

    let vars = Variables::with_overrides(None, &[("tenant".to_string(), serde_json::json!("42"))])?;
    let content = helper
        .build_script("maintenance/reindex", PinMode::Auto, vars)
        .await?;

    // A JSON string override is escaped as a SQL literal, so the formatter
    // quotes it; minijinja eats the newline after the include's block tag.
    assert_eq!(content, "-- tenant '42'\nSELECT touch_tenant();");

    Ok(())
}

/// Pinning a script freezes the components it includes, but not the script
/// itself: it lives outside the pinned tree and is always read live.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_pinned_script_build_freezes_includes_but_not_the_script(
) -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;

    helper
        .write_component("util/touch.sql", "SELECT original_include();")
        .await?;
    helper
        .write_script(
            "maintenance/reindex",
            "-- original entry\n{% include \"util/touch.sql\" %}\n",
        )
        .await?;

    let hash = helper.pin_script("maintenance/reindex").await?;
    assert!(!hash.is_empty());

    let config = helper.load_config().await?;
    let config_path = config.pather().script_config_path("maintenance/reindex");
    assert_eq!(config_path, "/db/scripts/maintenance/reindex/config.toml");
    assert!(helper.fs.exists(&config_path).await?);

    // The config lives outside the component tree it records, so re-pinning
    // an unchanged tree is stable.
    let second_hash = helper.pin_script("maintenance/reindex").await?;
    assert_eq!(
        hash, second_hash,
        "pinning an unchanged tree must be stable"
    );

    helper
        .write_component("util/touch.sql", "SELECT edited_include();")
        .await?;
    helper
        .write_script(
            "maintenance/reindex",
            "-- edited entry\n{% include \"util/touch.sql\" %}\n",
        )
        .await?;

    // A pinned script builds against its pin without being asked to.
    let pinned_content = helper
        .build_script("maintenance/reindex", PinMode::Auto, None)
        .await?;
    assert_eq!(
        pinned_content, "-- edited entry\nSELECT original_include();",
        "a pinned build freezes the included component but reads the script live"
    );

    let live_content = helper
        .build_script("maintenance/reindex", PinMode::Disabled, None)
        .await?;
    assert_eq!(
        live_content, "-- edited entry\nSELECT edited_include();",
        "--no-pin must show the current components"
    );

    Ok(())
}

/// Scripts sit beside `components/` rather than inside it so that adding,
/// editing or pinning one leaves every migration's pin hash untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_scripts_do_not_perturb_a_migrations_pin() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;

    helper
        .write_component("util/touch.sql", "SELECT touch();")
        .await?;
    helper
        .write_migration("20260101000000-demo", "{% include \"util/touch.sql\" %}\n")
        .await?;

    let before = helper.pin_migration("20260101000000-demo").await?;

    helper
        .write_script("maintenance/reindex", "REINDEX DATABASE postgres;\n")
        .await?;
    helper.pin_script("maintenance/reindex").await?;

    let after = helper.pin_migration("20260101000000-demo").await?;
    assert_eq!(
        before, after,
        "a script must not change the component root a migration pins"
    );

    // Contrast: a real component edit does change it, so the check above is
    // not vacuous.
    helper
        .write_component("util/touch.sql", "SELECT touched_differently();")
        .await?;
    let edited = helper.pin_migration("20260101000000-demo").await?;
    assert_ne!(
        before, edited,
        "editing a component must still change the pin"
    );

    Ok(())
}

/// A script's config.toml is a pin reference like a migration's lock file,
/// so collection must not treat its root as orphaned.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_pin_cleanup_keeps_a_script_only_pin() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;

    helper
        .write_component("util/touch.sql", "SELECT touch_tenant();")
        .await?;
    helper
        .write_script("maintenance/reindex", "{% include \"util/touch.sql\" %}\n")
        .await?;
    helper.pin_script("maintenance/reindex").await?;

    let files_before = helper.collect_all_files().await?;
    let config = helper.load_config().await?;

    let outcome = PinCleanup { dry_run: false }.execute(&config).await?;
    let Outcome::PinCleanup {
        orphaned,
        referenced_count,
        ..
    } = outcome
    else {
        panic!("Expected Outcome::PinCleanup");
    };

    assert!(
        orphaned.is_empty(),
        "a script-only pin must not be collected: {:?}",
        orphaned
    );
    assert!(referenced_count > 0, "the script's root must be counted");
    assert_eq!(
        files_before,
        helper.collect_all_files().await?,
        "nothing should have been deleted"
    );

    let content = helper
        .build_script("maintenance/reindex", PinMode::Required, None)
        .await?;
    assert_eq!(content, "SELECT touch_tenant();");

    Ok(())
}

/// An unpinned script builds against the current components without being
/// asked to, so `--no-pin` is never needed just to build one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_an_unpinned_script_builds_against_current_components(
) -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;

    helper
        .write_component("util/touch.sql", "SELECT live();")
        .await?;
    helper
        .write_script("maintenance/reindex", "{% include \"util/touch.sql\" %}\n")
        .await?;

    let content = helper
        .build_script("maintenance/reindex", PinMode::Auto, None)
        .await?;
    assert_eq!(content, "SELECT live();");

    Ok(())
}

/// `--pinned` is an assertion: a script with no pin fails rather than
/// quietly falling back to the current components.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_requiring_a_pin_fails_on_an_unpinned_script() -> Result<(), Box<dyn std::error::Error>>
{
    let helper = MigrationTestHelper::new_empty().await?;

    helper
        .write_component("util/touch.sql", "SELECT live();")
        .await?;
    helper
        .write_script("maintenance/reindex", "{% include \"util/touch.sql\" %}\n")
        .await?;

    let err = helper
        .build_script("maintenance/reindex", PinMode::Required, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("is not pinned"), "got: {}", err);

    Ok(())
}

/// `pin verify` reports a script whose recorded root has gone missing.
#[tokio::test]
async fn test_pin_verify_reports_a_missing_script_root() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;

    helper
        .write_component("util/touch.sql", "SELECT 1;\n")
        .await?;
    helper
        .write_script("maintenance/reindex", "{% include \"util/touch.sql\" %}\n")
        .await?;
    let hash = helper.pin_script("maintenance/reindex").await?;

    let config = helper.load_config().await?;
    let root_path = format!(
        "{}/{}/{}",
        config.pather().pinned_folder(),
        &hash[..2],
        &hash[2..]
    );
    helper.fs.delete(&root_path).await?;

    let outcome = PinVerify.execute(&config).await?;
    let Outcome::PinVerify {
        missing_root_count, ..
    } = outcome
    else {
        panic!("Expected Outcome::PinVerify");
    };
    assert_eq!(missing_root_count, 1);

    Ok(())
}

/// Naming a script that isn't there says so, rather than failing deeper in
/// the render.
#[tokio::test]
async fn test_script_build_rejects_an_unknown_script() -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;

    let err = helper
        .build_script("maintenance/nope", PinMode::Auto, None)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("not found"), "got: {}", err);

    Ok(())
}

// --- A migration's lock.toml without a pin -----------------------------------
//
// `lock.toml` records a pin *and* whatever variables the migration declares,
// and either may be absent. These cover the half that was previously
// impossible: a lock file that pins nothing.

/// A lock.toml holding only declarations pins nothing, so the build resolves
/// against the current components and still sees the declared value.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_a_migration_may_declare_variables_without_being_pinned(
) -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;
    let migration = helper.create_migration("declares-variables").await?;
    helper
        .write_migration(&migration, "SELECT {{ variables.tenant }};\n")
        .await?;
    helper
        .write_migration_lock(
            &migration,
            r#"[variables.tenant]
source = "value"
value = 42
"#,
        )
        .await?;

    let built = helper.build_migration(&migration, false).await?;
    assert_eq!(built, "SELECT 42;");

    Ok(())
}

/// The pin is what makes a migration pinned, never the file's existence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_requiring_a_pin_fails_on_a_variables_only_lock(
) -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;
    let migration = helper.create_migration("declares-variables").await?;
    helper
        .write_migration_lock(
            &migration,
            "[variables.tenant]\nsource = \"value\"\nvalue = 42\n",
        )
        .await?;

    let err = helper
        .build_migration(&migration, true)
        .await
        .expect_err("a lock.toml with no pin must not count as pinned");
    let msg = format!("{:#}", err);
    assert!(
        msg.contains("is not pinned"),
        "expected a 'not pinned' error, got: {}",
        msg
    );

    Ok(())
}

/// `check` reports the same thing, rather than being satisfied by the file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_check_reports_a_variables_only_lock_as_unpinned(
) -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;
    let migration = helper.create_migration("declares-variables").await?;
    helper
        .write_migration_lock(
            &migration,
            "[variables.tenant]\nsource = \"value\"\nvalue = 1\n",
        )
        .await?;

    let config = helper.load_config().await?;
    assert!(matches!(
        Check.execute(&config).await?,
        Outcome::CheckFailed
    ));

    Ok(())
}

/// Pinning rewrites lock.toml in place, so declarations already in it survive.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_pinning_a_migration_keeps_its_declared_variables(
) -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;
    let migration = helper.create_migration("declares-variables").await?;
    helper
        .write_migration(&migration, "SELECT {{ variables.tenant }};\n")
        .await?;
    helper
        .write_migration_lock(
            &migration,
            r#"[variables.tenant]
source = "value"
value = 42
"#,
        )
        .await?;

    let hash = helper.pin_migration(&migration).await?;

    let lock = helper.read_migration_lock(&migration).await?;
    assert!(
        lock.contains(&format!("pin = \"{}\"", hash)),
        "pin should be recorded, got: {}",
        lock
    );
    assert!(
        lock.contains("[variables.tenant]"),
        "declarations should survive pinning, got: {}",
        lock
    );

    // And the migration now builds pinned, still seeing the declared value.
    assert_eq!(
        helper.build_migration(&migration, true).await?,
        "SELECT 42;"
    );

    Ok(())
}

/// A declared prompt default is taken under `--use-defaults`, as for a script.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_a_declared_migration_default_is_taken_with_use_defaults(
) -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;
    let migration = helper.create_migration("prompts").await?;
    helper
        .write_migration(&migration, "SELECT {{ variables.batch }};\n")
        .await?;
    helper
        .write_migration_lock(
            &migration,
            r#"[variables.batch]
source = "prompt"
type = "number"
default = 7
"#,
        )
        .await?;

    assert_eq!(
        helper
            .build_migration_with_defaults(&migration, false)
            .await?,
        "SELECT 7;"
    );

    Ok(())
}

/// A declaration is the lowest-priority source, so the command line wins.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_var_json_beats_a_declared_migration_variable(
) -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;
    let migration = helper.create_migration("declares-variables").await?;
    helper
        .write_migration(&migration, "SELECT {{ variables.tenant }};\n")
        .await?;
    helper
        .write_migration_lock(
            &migration,
            "[variables.tenant]\nsource = \"value\"\nvalue = 1\n",
        )
        .await?;

    let built = helper
        .build_migration_with_variable_overrides(
            &migration,
            false,
            None,
            &[("tenant".to_string(), serde_json::json!(99))],
        )
        .await?;
    assert_eq!(built, "SELECT 99;");

    Ok(())
}

/// A pin recorded beside declarations is still a pin: cleanup must walk it
/// rather than treat the lock as variables-only and collect its tree.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_pin_cleanup_keeps_a_pin_recorded_beside_declared_variables(
) -> Result<(), Box<dyn std::error::Error>> {
    let helper = MigrationTestHelper::new_empty().await?;
    helper
        .write_component("greeting.sql", "SELECT 'hello';\n")
        .await?;
    let migration = helper.create_migration("declares-variables").await?;
    helper
        .write_migration_lock(
            &migration,
            "[variables.tenant]\nsource = \"value\"\nvalue = 1\n",
        )
        .await?;
    helper.pin_migration(&migration).await?;

    let config = helper.load_config().await?;
    let files_before = helper.collect_all_files().await?;

    let Outcome::PinCleanup {
        orphaned,
        referenced_count,
        ..
    } = (PinCleanup { dry_run: false }).execute(&config).await?
    else {
        panic!("Expected Outcome::PinCleanup");
    };

    assert!(
        orphaned.is_empty(),
        "nothing should be orphaned: {:?}",
        orphaned
    );
    assert!(referenced_count > 0, "the pin's tree should be referenced");
    assert_eq!(
        files_before,
        helper.collect_all_files().await?,
        "cleanup should not touch a referenced tree"
    );

    Ok(())
}
