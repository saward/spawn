use anyhow::Result;
use std::fs;
use std::{path::PathBuf, str::FromStr};

use serde::{Deserialize, Serialize};

pub const MIGRATION_FILE: &str = "migrator.toml";

// A single file entry with its hash.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    pub db_connstring: String,
    pub scripts_path: PathBuf,

    #[serde(default = "default_environment")]
    pub environment: String,
}

fn default_environment() -> String {
    "prod".to_string()
}

impl Config {
    pub fn load() -> Result<Config> {
        let config_file = PathBuf::from_str(MIGRATION_FILE)?;
        let contents = fs::read_to_string(config_file)?;

        let config: Config = toml::from_str(&contents)?;

        Ok(config)
    }
}
