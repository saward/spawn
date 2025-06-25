use crate::dbdriver::{postgres_psql::PSQL, Database};
use crate::pinfile::LockData;
use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

static PINFILE_LOCK_NAME: &str = "lock.toml";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub spawn_folder: PathBuf,
    pub default_database: String,

    #[serde(default = "default_environment")]
    pub environment: String,

    pub databases: HashMap<String, DatabaseConfig>,
}

impl Config {
    pub fn new_driver(&self) -> Result<Box<dyn Database>> {
        let db_config = self.databases.get(&self.default_database).ok_or(anyhow!(
            "no database defined with name '{}'",
            &self.default_database
        ))?;

        match db_config.driver.as_str() {
            "postgres-psql" => Ok(PSQL::new(&db_config.command.clone().ok_or(anyhow!(
                "command must be specified for driver {}",
                &db_config.driver
            ))?)),
            _ => Err(anyhow!(
                "no driver with name '{}' exists",
                &db_config.driver
            )),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DatabaseConfig {
    pub driver: String,

    #[serde(default)]
    pub command: Option<Vec<String>>,
}

fn default_environment() -> String {
    "prod".to_string()
}

impl Config {
    pub fn load(path: &str, database: Option<String>) -> Result<Config> {
        let mut settings: Config = config::Config::builder()
            .add_source(config::File::with_name(path))
            // Used to override the version in a repo with your own custom local overrides.
            .add_source(config::File::with_name("spawn.override.toml").required(false))
            // Add in settings from the environment (with a prefix of APP)
            // Eg.. `APP_DEBUG=1 ./target/app` would set the `debug` key
            .add_source(config::Environment::with_prefix("SPAWN"))
            .set_default("environment", "prod")
            .context("could not set default environment")?
            .build()?
            .try_deserialize()
            .context("could not deserialise config struct")?;

        if let Some(db) = database {
            settings.default_database = db;
        }

        Ok(settings)
    }

    pub fn pinned_folder(&self) -> PathBuf {
        self.spawn_folder.join("pinned")
    }

    pub fn components_folder(&self) -> PathBuf {
        self.spawn_folder.join("components")
    }

    pub fn migrations_folder(&self) -> PathBuf {
        self.spawn_folder.join("migrations")
    }

    pub fn tests_folder(&self) -> PathBuf {
        self.spawn_folder.join("tests")
    }

    pub fn migration_folder(&self, script_path: &OsString) -> PathBuf {
        self.migrations_folder().join(script_path)
    }

    pub fn migration_script_file_path(&self, script_path: &OsString) -> PathBuf {
        self.migration_folder(script_path).join("script.sql")
    }

    pub fn test_folder(&self, test_path: &OsString) -> PathBuf {
        self.tests_folder().join(test_path)
    }

    pub fn test_file_path(&self, test_path: &OsString) -> PathBuf {
        self.test_folder(test_path).join("test.sql")
    }

    pub fn migration_lock_file_path(&self, script_path: &OsString) -> PathBuf {
        // Nightly has an add_extension that might be good to use one day if it
        // enters stable.
        let mut lock_file_name = script_path.clone();
        lock_file_name.push(PINFILE_LOCK_NAME);

        self.migrations_folder()
            .join(script_path)
            .join(PINFILE_LOCK_NAME)
    }

    pub fn load_lock_file(&self, lock_file_path: &PathBuf) -> Result<LockData> {
        let contents = fs::read_to_string(lock_file_path)?;
        let lock_data: LockData = toml::from_str(&contents)?;

        Ok(lock_data)
    }
}
