use crate::engine::{postgres_psql::PSQL, DatabaseConfig, Engine};
use crate::pinfile::LockData;
use anyhow::{anyhow, Context, Result};
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
    pub fn spawn_folder_path(&self) -> &str {
        self.spawn_folder.as_ref()
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

    pub fn pinned_folder(&self) -> String {
        let mut s = self.spawn_folder_path().to_string();
        s.push_str("/pinned");
        s
    }

    pub fn components_folder(&self) -> String {
        let mut s = self.spawn_folder_path().to_string();
        s.push_str("/components");
        s
    }

    pub fn migrations_folder(&self) -> String {
        let mut s = self.spawn_folder_path().to_string();
        s.push_str("/migrations");
        s
    }

    pub fn tests_folder(&self) -> String {
        let mut s = self.spawn_folder_path().to_string();
        s.push_str("/tests");
        s
    }

    pub fn migration_folder(&self, script_path: &str) -> String {
        let mut s = self.migrations_folder();
        s.push('/');
        s.push_str(script_path);
        s
    }

    pub fn migration_script_file_path(&self, script_path: &str) -> String {
        let mut s = self.migration_folder(script_path);
        s.push_str("/up.sql");
        s
    }

    pub fn test_folder(&self, test_path: &str) -> String {
        let mut s = self.tests_folder();
        s.push('/');
        s.push_str(test_path);
        s
    }

    pub fn test_file_path(&self, test_path: &str) -> String {
        let mut s = self.test_folder(test_path);
        s.push_str("/test.sql");
        s
    }

    pub fn migration_lock_file_path(&self, script_path: &str) -> String {
        let mut s = self.migrations_folder();
        s.push('/');
        s.push_str(script_path);
        s.push('/');
        s.push_str(PINFILE_LOCK_NAME);
        s
    }

    pub fn load_lock_file(&self, lock_file_path: &str) -> Result<LockData> {
        let contents = fs::read_to_string(lock_file_path)?;
        let lock_data: LockData = toml::from_str(&contents)?;

        Ok(lock_data)
    }
}
