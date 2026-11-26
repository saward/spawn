use crate::config::Config;
use anyhow::Result;

pub mod check;
pub mod init;
pub mod migration;
pub mod pin_cleanup;
pub mod pin_verify;
pub mod script;
pub mod test;

pub use check::Check;
pub use init::Init;
pub use migration::{
    AdoptMigration, ApplyMigration, BuildMigration, DownMigration, MigrationStatus, NewMigration,
    PinError, PinMigration,
};
pub use pin_cleanup::PinCleanup;
pub use pin_verify::PinVerify;
pub use script::{BuildScript, NewScript, PinScript, PinScriptError, RunScript};
pub use test::{BuildTest, CompareTests, ExpectTest, NewTest, RunTest};

/// Formats the target and environment a command is about to act on, flagging production.
pub fn target_banner(config: &Config) -> Result<String> {
    let target_config = config.target_config()?;
    let target = config.target.as_deref().unwrap_or("unknown");
    let env = &target_config.environment;

    let environment = if env.starts_with("prod") {
        format!("ENVIRONMENT: {} \u{26a0}\u{fe0f}", env)
    } else {
        format!("ENVIRONMENT: {}", env)
    };

    Ok(format!("\nTARGET: {}\n{}", target, environment))
}

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
    BuiltMigration {
        content: String,
        pinned_warn: bool,
    },
    BuiltScript {
        content: String,
    },
    CheckFailed,
    PinCleanup {
        /// Hashes that were deleted (or would be deleted in dry-run mode).
        orphaned: Vec<String>,
        /// Total number of hashes that are still referenced.
        referenced_count: usize,
        /// Whether this was a dry run.
        dry_run: bool,
    },
    PinVerify {
        /// Number of pinned files whose contents no longer match their hash.
        corrupted_count: usize,
        /// Number of migrations or scripts whose root hash is missing from the store.
        missing_root_count: usize,
    },
    NewMigration(String),
    NewTest(String),
    PinnedMigration {
        hash: String,
    },
    NewScript {
        script: String,
        path: String,
    },
    PinnedScript {
        script: String,
        hash: String,
    },
    Success,
    Unimplemented,
}
