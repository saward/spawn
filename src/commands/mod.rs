use crate::config::Config;
use anyhow::Result;

pub mod init;
pub mod migration;
pub mod test;

pub use init::Init;
pub use migration::{ApplyMigration, BuildMigration, NewMigration, PinMigration};
pub use test::{BuildTest, CompareTests, ExpectTest, RunTest};

/// Trait for describing commands in a telemetry-safe way.
///
/// Implementations should return sanitized command strings that don't
/// contain sensitive information like file paths or migration names.
pub trait TelemetryDescribe {
    /// Returns a sanitized command string for telemetry.
    /// Should not contain sensitive values like file paths or migration names.
    fn telemetry_command(&self) -> String;

    /// Returns additional safe properties to include in telemetry.
    /// Only include non-sensitive boolean flags or enum values.
    fn telemetry_properties(&self) -> Vec<(&'static str, String)> {
        vec![]
    }
}

/// Trait for executable commands. All commands must also implement TelemetryDescribe.
#[allow(async_fn_in_trait)]
pub trait Command: TelemetryDescribe {
    async fn execute(&self, config: &Config) -> Result<Outcome>;
}

pub enum Outcome {
    NewMigration(String),
    BuiltMigration { content: String },
    AppliedMigrations,
    Unimplemented,
    PinnedMigration { hash: String },
    Success,
}
