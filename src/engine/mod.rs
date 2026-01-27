use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;
use tokio::process::Command;

pub mod postgres_psql;

/// Status of a migration in the tracking tables
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationStatus {
    Success,
    Attempted,
    Failure,
}

impl MigrationStatus {
    /// Returns the string representation used in the database
    pub fn as_str(&self) -> &'static str {
        match self {
            MigrationStatus::Success => "SUCCESS",
            MigrationStatus::Attempted => "ATTEMPTED",
            MigrationStatus::Failure => "FAILURE",
        }
    }

    /// Parse a MigrationStatus from a string representation
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "SUCCESS" => Some(MigrationStatus::Success),
            "ATTEMPTED" => Some(MigrationStatus::Attempted),
            "FAILURE" => Some(MigrationStatus::Failure),
            _ => None,
        }
    }
}

impl fmt::Display for MigrationStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Activity type for a migration operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationActivity {
    Apply,
    Adopt,
    Revert,
}

impl MigrationActivity {
    /// Returns the string representation used in the database
    pub fn as_str(&self) -> &'static str {
        match self {
            MigrationActivity::Apply => "APPLY",
            MigrationActivity::Adopt => "ADOPT",
            MigrationActivity::Revert => "REVERT",
        }
    }
}

impl fmt::Display for MigrationActivity {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Legacy type alias for backwards compatibility
pub type MigrationHistoryStatus = MigrationStatus;

/// Information about an existing migration entry
#[derive(Debug, Clone)]
pub struct ExistingMigrationInfo {
    pub migration_name: String,
    pub namespace: String,
    pub last_status: MigrationHistoryStatus,
    pub last_activity: String,
    pub checksum: String,
}

/// Database information about a migration
#[derive(Debug, Clone)]
pub struct MigrationDbInfo {
    pub migration_name: String,
    pub last_status: Option<MigrationHistoryStatus>,
    pub last_activity: Option<String>,
    pub checksum: Option<String>,
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

    // Could not get advisory lock
    #[error("could not get advisory lock: {0}")]
    AdvisoryLock(std::io::Error),

    /// CRITICAL: Migration executed successfully but recording to migration tables failed.
    /// The database is now in an inconsistent state - the migration has been applied
    /// but spawn has no record of it. Manual intervention is required.
    #[error(
        "\n\
        ********************************************************************************\n\
        *                         CRITICAL ERROR - MANUAL INTERVENTION REQUIRED                          *\n\
        ********************************************************************************\n\
        \n\
        Migration '{name}' in namespace '{namespace}' was SUCCESSFULLY APPLIED to the database,\n\
        but FAILED to record in spawn's migration tracking tables.\n\
        \n\
        YOUR DATABASE IS NOW IN AN INCONSISTENT STATE.\n\
        \n\
        The migration changes ARE in your database, but spawn does not know about them.\n\
        If you retry this migration, it may cause errors or duplicate changes.\n\
        \n\
        Recording error: {recording_error}\n\
        \n\
        TO RESOLVE:\n\
        1. Verify the migration was applied by checking your database schema\n\
        2. Run `spawn migration adopt <name>` to record the migration as adopted\n\
        3. Investigate why the recording failed (connection issue? permissions?)\n\
        \n\
        ********************************************************************************\n"
    )]
    MigrationAppliedButNotRecorded {
        name: String,
        namespace: String,
        schema: String,
        recording_error: String,
    },
}

/// Result type for migration operations
pub type MigrationResult<T> = Result<T, MigrationError>;

/// Errors for streaming SQL execution
#[derive(Debug, Error)]
pub enum EngineError {
    #[error("execution failed (exit {exit_code}): {stderr}")]
    ExecutionFailed { exit_code: i32, stderr: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

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

/// Specifies how to obtain the command to execute for database operations.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandSpec {
    /// Direct command to execute
    Direct { direct: Vec<String> },
    /// A provider command that outputs the actual command as JSON
    Provider {
        provider: Vec<String>,
        #[serde(default)]
        append: Vec<String>,
    },
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
    pub command: Option<CommandSpec>,
}

fn default_environment() -> String {
    "prod".to_string()
}

fn default_schema() -> String {
    "_spawn".to_string()
}

/// Resolves a CommandSpec to the actual command to execute.
///
/// This is a generic function that can be used by any database engine
/// to resolve their command specification.
pub async fn resolve_command_spec(spec: CommandSpec) -> Result<Vec<String>> {
    match spec {
        CommandSpec::Direct { direct } => Ok(direct),
        CommandSpec::Provider { provider, append } => {
            let mut resolved = resolve_provider(&provider).await?;
            resolved.extend(append);
            Ok(resolved)
        }
    }
}

/// Executes a provider command and parses its output as a shell command.
///
/// The provider must output a shell command string (e.g., `ssh -t -i /path/key user@host`).
/// The parser handles quoted strings properly using POSIX shell-style parsing.
async fn resolve_provider(provider: &[String]) -> Result<Vec<String>> {
    if provider.is_empty() {
        return Err(anyhow!("Provider command cannot be empty"));
    }

    let output = Command::new(&provider[0])
        .args(&provider[1..])
        .output()
        .await
        .context("Failed to execute provider command")?;

    if !output.status.success() {
        return Err(anyhow!(
            "Provider command failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout = String::from_utf8(output.stdout).context("Provider output is not valid UTF-8")?;
    let trimmed = stdout.trim();

    if trimmed.is_empty() {
        return Err(anyhow!("Provider returned empty output"));
    }

    // Parses a shell command string into a Vec<String>, handling quoted arguments.
    //
    // Uses the `shlex` crate for proper POSIX shell-style parsing.
    shlex::split(trimmed).ok_or_else(|| anyhow!("Failed to parse shell command: {}", trimmed))
}

/// Type alias for the writer closure used in execute_with_writer
pub type WriterFn = Box<dyn FnOnce(&mut dyn std::io::Write) -> std::io::Result<()> + Send>;

/// Type alias for an optional stdout writer to capture output
pub type StdoutWriter = Option<Box<dyn tokio::io::AsyncWrite + Send + Unpin>>;

#[async_trait]
pub trait Engine: Send + Sync {
    /// Execute SQL by running the provided writer function.
    /// - `write_fn`: Closure that writes SQL to the provided Write handle
    /// - `stdout_writer`: Optional writer to capture stdout. If None, stdout is discarded.
    /// Engine-specific setup (like psql flags) is handled internally.
    /// Returns stderr content on failure.
    async fn execute_with_writer(
        &self,
        write_fn: WriterFn,
        stdout_writer: StdoutWriter,
    ) -> Result<(), EngineError>;

    async fn migration_apply(
        &self,
        migration_name: &str,
        write_fn: WriterFn,
        pin_hash: Option<String>,
        namespace: &str,
    ) -> MigrationResult<String>;

    /// Adopt a migration without applying it.
    /// Creates a dummy table entry marking the migration as having been applied manually.
    /// Sets checksum to empty and status to 'SUCCESS'.
    async fn migration_adopt(
        &self,
        migration_name: &str,
        namespace: &str,
        description: &str,
    ) -> MigrationResult<String>;

    /// Get database information for all migrations in the given namespace.
    /// If namespace is None, returns migrations from all namespaces.
    /// Returns a list of migrations that exist in the database with their latest history entry.
    async fn get_migrations_from_db(
        &self,
        namespace: Option<&str>,
    ) -> MigrationResult<Vec<MigrationDbInfo>>;
}
