use crate::pinfile::LockData;
use anyhow::{Context, Result};
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const CONFIG_FILE_NAME: &str = "spawn.toml";
static PINFILE_LOCK_NAME: &str = "lock.toml";

// A single file entry with its hash.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub db_connstring: String,
    pub scripts_path: PathBuf,

    #[serde(default = "default_environment")]
    pub environment: String,

    pub psql_command: Vec<String>,
}

fn default_environment() -> String {
    "prod".to_string()
}

impl Config {
    pub fn load() -> Result<Config> {
        let settings: Config = config::Config::builder()
            .add_source(config::File::with_name("spawn.toml"))
            // Used to override the version in a repo with your own custom local overrides.
            .add_source(config::File::with_name("spawn.override.toml").required(false))
            // Add in settings from the environment (with a prefix of APP)
            // Eg.. `APP_DEBUG=1 ./target/app` would set the `debug` key
            .add_source(config::Environment::with_prefix("SPAWN"))
            .set_default("environment", "prod")
            .context("could not set default environment")?
            .build()
            .unwrap()
            .try_deserialize()
            .context("could not deserialise config struct")?;

        println!("db con string: {}", settings.db_connstring);

        Ok(settings)
    }

    pub fn pinned_folder(&self) -> PathBuf {
        self.scripts_path.join("pinned")
    }

    pub fn components_folder(&self) -> PathBuf {
        self.scripts_path.join("components")
    }

    pub fn migrations_folder(&self) -> PathBuf {
        self.scripts_path.join("migrations")
    }

    pub fn tests_folder(&self) -> PathBuf {
        self.scripts_path.join("tests")
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
