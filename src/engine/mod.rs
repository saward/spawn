use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{fmt, io};
use thiserror::Error;

pub mod postgres_psql;

/// Status of a previous migration attempt
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationHistoryStatus {
    Success,
    Attempted,
    Failure,
}

impl fmt::Display for MigrationHistoryStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MigrationHistoryStatus::Success => write!(f, "SUCCESS"),
            MigrationHistoryStatus::Attempted => write!(f, "ATTEMPTED"),
            MigrationHistoryStatus::Failure => write!(f, "FAILURE"),
        }
    }
}

/// Information about an existing migration entry
#[derive(Debug, Clone)]
pub struct ExistingMigrationInfo {
    pub migration_name: String,
    pub namespace: String,
    pub last_status: MigrationHistoryStatus,
    pub last_activity: String,
    pub checksum: String,
}

/// Errors specific to migration operations
#[derive(Debug, Error)]
pub enum MigrationError {
    /// Migration was already successfully applied
    #[error("migration '{name}' in namespace '{namespace}' already applied successfully")]
    AlreadyApplied {
        name: String,
        namespace: String,
        info: ExistingMigrationInfo,
    },

    /// Migration exists but last attempt was not successful
    #[error("migration '{name}' in namespace '{namespace}' has previous {status} status")]
    PreviousAttemptFailed {
        name: String,
        namespace: String,
        status: MigrationHistoryStatus,
        info: ExistingMigrationInfo,
    },

    /// Database or connection error
    #[error("database error: {0}")]
    Database(#[from] anyhow::Error),
}

/// Result type for migration operations
pub type MigrationResult<T> = Result<T, MigrationError>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum EngineType {
    #[serde(rename = "postgres-psql")]
    PostgresPSQL,
}

impl fmt::Display for EngineType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            EngineType::PostgresPSQL => {
                write!(f, "postgres-psql")
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DatabaseConfig {
    pub engine: EngineType,
    pub spawn_database: String,
    #[serde(default = "default_schema")]
    pub spawn_schema: String,
    #[serde(default = "default_environment")]
    pub environment: String,

    #[serde(default)]
    pub command: Option<Vec<String>>,
}

fn default_environment() -> String {
    "prod".to_string()
}

fn default_schema() -> String {
    "_spawn".to_string()
}

pub struct MigrationStatus {
    _applied: bool,
}

pub struct EngineStatus {
    _connection_successful: Option<bool>,
}

pub trait EngineOutputter {
    fn output(&mut self) -> io::Result<Vec<u8>>;
}

pub trait EngineWriter: io::Write {
    // finalise consumes self so that no more writing can be done after trying
    // to fetch output.
    fn finalise(self: Box<Self>) -> anyhow::Result<Box<dyn EngineOutputter>>;
}

#[async_trait]
pub trait Engine {
    /// Provides a writer that a given migration can be sent to, so that we can
    /// stream data to this as we go.  May not be implemented for all engines.
    fn new_writer(&self) -> anyhow::Result<Box<dyn EngineWriter>>;

    async fn migration_apply(
        &self,
        migration_name: &str,
        migration: &str,
        pin_hash: Option<String>,
        namespace: &str,
    ) -> MigrationResult<String>;

    // /// Return information about this migration, such as whether it has been
    // /// applied.
    // fn migration_status(&self, checksum: &[u8]) -> anyhow::Result<Status>;

    // /// Performs a check on the connection to see
    // fn check(&self) -> Result<EngineStatus>;
}
