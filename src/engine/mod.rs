use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::io;

pub mod postgres_psql;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DatabaseConfig {
    pub engine: String,
    pub spawn_database: String,
    #[serde(default = "default_schema")]
    pub spawn_schema: String,
    #[serde(default = "default_environment")]
    pub environment: String,

    #[serde(default)]
    pub command: Option<Vec<String>>,
}

fn default_environment() -> String {
    "dev".to_string()
}

fn default_schema() -> String {
    "_spawn".to_string()
}

pub struct MigrationStatus {
    applied: bool,
}

pub struct EngineStatus {
    connection_successful: Option<bool>,
}

pub trait EngineOutputter {
    fn output(&mut self) -> io::Result<Vec<u8>>;
}

pub trait EngineWriter: io::Write {
    // finalise consumes self so that no more writing can be done after trying
    // to fetch output.
    fn finalise(self: Box<Self>) -> Result<Box<dyn EngineOutputter>>;
}

#[async_trait]
pub trait Engine {
    /// Provides a writer that a given migration can be sent to, so that we can
    /// stream data to this as we go.  May not be implemented for all engines.
    fn new_writer(&self) -> Result<Box<dyn EngineWriter>>;

    async fn migration_apply(&self, migration: &str) -> Result<String>;

    // /// Return information about this migration, such as whether it has been
    // /// applied.
    // fn migration_status(&self, checksum: &[u8]) -> anyhow::Result<Status>;

    // /// Performs a check on the connection to see
    // fn check(&self) -> Result<EngineStatus>;
}
