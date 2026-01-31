use crate::config::Config;
use anyhow::Result;

pub mod check;
pub mod init;
pub mod migration;
pub mod test;

pub use check::Check;
pub use init::Init;
pub use migration::{
    AdoptMigration, ApplyMigration, BuildMigration, MigrationStatus, NewMigration, PinMigration,
};
pub use test::{BuildTest, CompareTests, ExpectTest, NewTest, RunTest};

/// Telemetry information for a command.
#[derive(Debug, Clone, Default)]
pub struct TelemetryInfo {
    /// A sanitized label for the command (e.g., "migration build").
    /// Should not contain sensitive values like file paths or migration names.
    pub label: String,
    /// Additional safe properties to include in telemetry.
    /// Only include non-sensitive boolean flags or enum values.
    pub properties: Vec<(&'static str, String)>,
}

impl TelemetryInfo {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            properties: vec![],
        }
    }

    pub fn with_properties(mut self, properties: Vec<(&'static str, String)>) -> Self {
        self.properties = properties;
        self
    }
}

/// Trait for describing commands in a telemetry-safe way.
///
/// Implementations should return sanitized info that doesn't
/// contain sensitive information like file paths or migration names.
pub trait TelemetryDescribe {
    fn telemetry(&self) -> TelemetryInfo;
}

/// Trait for executable commands. All commands must also implement TelemetryDescribe.
#[allow(async_fn_in_trait)]
pub trait Command: TelemetryDescribe {
    async fn execute(&self, config: &Config) -> Result<Outcome>;
}

pub enum Outcome {
    AdoptedMigration,
    AppliedMigrations,
    BuiltMigration { content: String, pinned_warn: bool },
    CheckFailed,
    NewMigration(String),
    NewTest(String),
    PinnedMigration { hash: String },
    Success,
    Unimplemented,
}
