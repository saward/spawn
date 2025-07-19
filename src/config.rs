use crate::engine::{postgres_psql::PSQL, DatabaseConfig, Engine};
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
    pub database: String,
    pub environment: Option<String>, // Override the environment for the db config

    pub databases: HashMap<String, DatabaseConfig>,
}

impl Config {
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
