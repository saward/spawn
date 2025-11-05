use crate::engine::{postgres_psql::PSQL, DatabaseConfig, Engine};
use crate::pinfile::LockData;
use anyhow::{anyhow, Context, Result};
use object_store::path::Path;
use std::collections::HashMap;

use std::fs;

use serde::{Deserialize, Serialize};

static PINFILE_LOCK_NAME: &str = "lock.toml";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    spawn_folder: String,
    pub database: String,
    pub environment: Option<String>, // Override the environment for the db config

    pub databases: HashMap<String, DatabaseConfig>,
}

impl Config {
    pub fn spawn_folder_path(&self) -> Path {
        Path::from(self.spawn_folder.clone())
    }

    pub fn new_engine(&self) -> Result<Box<dyn Engine>> {
        let db_config = self.db_config()?;

        match db_config.engine.as_str() {
            "postgres-psql" => Ok(PSQL::new(&db_config)?),
            _ => Err(anyhow!(
                "no engine with name '{}' exists",
                &db_config.engine
            )),
        }
    }

    pub fn db_config(&self) -> Result<DatabaseConfig> {
        let mut conf = self
            .databases
            .get(&self.database)
            .ok_or(anyhow!(
                "no database defined with name '{}'",
                &self.database
            ))?
            .clone();

        if let Some(env) = &self.environment {
            conf.environment = env.clone();
        }

        Ok(conf)
    }
}

impl Config {
    pub fn load(path: &str, database: Option<String>) -> Result<Config> {
        let settings: Config = config::Config::builder()
            .add_source(config::File::with_name(path))
            // Used to override the version in a repo with your own custom local overrides.
            .add_source(config::File::with_name("spawn.override.toml").required(false))
            // Add in settings from the environment (with a prefix of APP)
            // Eg.. `APP_DEBUG=1 ./target/app` would set the `debug` key
            .add_source(config::Environment::with_prefix("SPAWN"))
            .set_override_option("database", database)?
            .set_default("environment", "prod")
            .context("could not set default environment")?
            .build()?
            .try_deserialize()
            .context("could not deserialise config struct")?;

        Ok(settings)
    }

    pub fn pinned_folder(&self) -> Path {
        self.spawn_folder_path().child("/pinned")
    }

    pub fn components_folder(&self) -> Path {
        self.spawn_folder_path().child("/components")
    }

    pub fn migrations_folder(&self) -> Path {
        self.spawn_folder_path().child("/migrations")
    }

    pub fn tests_folder(&self) -> Path {
        self.spawn_folder_path().child("/tests")
    }

    pub fn migration_folder(&self, script_path: &Path) -> Path {
        self.migrations_folder().child(script_path.as_ref())
    }

    pub fn migration_script_file_path(&self, script_path: &Path) -> Path {
        self.migration_folder(script_path).child("up.sql")
    }

    pub fn test_folder(&self, test_path: &Path) -> Path {
        self.tests_folder().child(test_path.as_ref())
    }

    pub fn test_file_path(&self, test_path: &Path) -> Path {
        self.test_folder(test_path).child("test.sql")
    }

    pub fn migration_lock_file_path(&self, script_path: &Path) -> Path {
        // Use object_store::Path for consistent path handling
        self.migrations_folder()
            .child(script_path.as_ref())
            .child(PINFILE_LOCK_NAME)
    }

    pub fn load_lock_file(&self, lock_file_path: &Path) -> Result<LockData> {
        let path_str = lock_file_path.as_ref();
        let contents = fs::read_to_string(path_str)?;
        let lock_data: LockData = toml::from_str(&contents)?;

        Ok(lock_data)
    }
}
