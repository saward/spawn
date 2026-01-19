//! Telemetry module for anonymous usage data collection.
//!
//! This module collects anonymous usage statistics to help improve spawn.
//! It is designed to be:
//! - **Non-blocking**: Telemetry runs in a background task
//! - **Privacy-respecting**: No personal data is collected
//! - **Fail-silent**: Errors are silently ignored
//!
//! ## Opt-out
//!
//! Telemetry can be disabled by:
//! 1. Setting the `DO_NOT_TRACK` environment variable (any value)
//! 2. Setting `telemetry = false` in `spawn.toml`

use posthog_rs::{ClientOptions, Event};
use std::env;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;

/// PostHog API key for spawn telemetry
const POSTHOG_API_KEY: &str = "phc_yD13QBdCJSnbIjmkTcSf03dRhpLJdCMfTVRzD7XTFqd";

/// Application version from Cargo.toml
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Global handle for the pending telemetry task
static PENDING_TASK: OnceLock<tokio::sync::Mutex<Option<JoinHandle<()>>>> = OnceLock::new();

/// Telemetry recorder for tracking command execution.
///
/// Use `TelemetryRecorder::new()` at the start of command execution,
/// then call `finish()` when the command completes.
pub struct TelemetryRecorder {
    enabled: bool,
    distinct_id: String,
    command: String,
    properties: Vec<(String, String)>,
    start_time: Instant,
}

/// Status of command execution for telemetry
#[derive(Clone, Copy, Debug)]
pub enum CommandStatus {
    Success,
    Error,
}

impl std::fmt::Display for CommandStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandStatus::Success => write!(f, "success"),
            CommandStatus::Error => write!(f, "error"),
        }
    }
}

impl TelemetryRecorder {
    /// Create a new telemetry recorder.
    ///
    /// Checks opt-out settings in priority order:
    /// 1. `DO_NOT_TRACK` env var -> Disable
    /// 2. `telemetry_enabled = false` -> Disable
    /// 3. Otherwise -> Enable
    ///
    /// If no `project_id` is provided, generates an ephemeral UUID for this session.
    pub fn new(
        project_id: Option<&str>,
        telemetry_enabled: bool,
        command: String,
        properties: Vec<(&str, String)>,
    ) -> Self {
        // Check DO_NOT_TRACK env var first (highest priority)
        let do_not_track = env::var("DO_NOT_TRACK").is_ok();

        // Determine if telemetry is enabled
        let enabled = !do_not_track && telemetry_enabled;

        // Get or generate distinct_id
        let distinct_id = if enabled {
            project_id
                .map(|s| s.to_string())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
        } else {
            String::new()
        };

        // Convert properties to owned strings
        let properties = properties
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();

        Self {
            enabled,
            distinct_id,
            command,
            properties,
            start_time: Instant::now(),
        }
    }

    /// Finish recording and spawn a background task to send telemetry.
    ///
    /// This method consumes the recorder and spawns a detached task.
    /// The task will be awaited (with timeout) when `flush()` is called.
    pub fn finish(self, status: CommandStatus, error_kind: Option<&str>) {
        if !self.enabled {
            return;
        }

        let duration_ms = self.start_time.elapsed().as_millis() as u64;
        let command = self.command;
        let distinct_id = self.distinct_id;
        let properties = self.properties;
        let error_kind = error_kind.map(|s| s.to_string());

        // Spawn background task
        let handle = tokio::spawn(async move {
            if let Err(_) = send_event(
                &distinct_id,
                &command,
                duration_ms,
                status,
                error_kind.as_deref(),
                properties,
            )
            .await
            {
                // Silently ignore errors - fail-silent design
            }
        });

        // Store handle for later flushing
        let mutex = PENDING_TASK.get_or_init(|| tokio::sync::Mutex::new(None));
        if let Ok(mut guard) = mutex.try_lock() {
            *guard = Some(handle);
        }
    }
}

/// Send the telemetry event to PostHog
async fn send_event(
    distinct_id: &str,
    command: &str,
    duration_ms: u64,
    status: CommandStatus,
    error_kind: Option<&str>,
    properties: Vec<(String, String)>,
) -> Result<(), posthog_rs::Error> {
    // Initialize the global PostHog client
    let options: ClientOptions = POSTHOG_API_KEY.into();
    // Note: init_global returns AlreadyInitialized if called twice, which is fine
    let _ = posthog_rs::init_global(options).await;

    let mut event = Event::new("command_completed", distinct_id);

    // Environment properties
    event.insert_prop("app_version", APP_VERSION)?;
    event.insert_prop("os_platform", std::env::consts::OS)?;
    event.insert_prop("os_arch", std::env::consts::ARCH)?;
    event.insert_prop("is_ci", is_ci())?;

    // Usage properties
    event.insert_prop("command", command)?;
    event.insert_prop("duration_ms", duration_ms)?;

    // Command-specific properties from TelemetryDescribe trait
    for (key, value) in properties {
        event.insert_prop(key, value)?;
    }

    // Health properties
    event.insert_prop("status", status.to_string())?;
    if let Some(kind) = error_kind {
        event.insert_prop("error_kind", kind)?;
    }

    posthog_rs::capture(event).await
}

/// Check if running in a CI environment
fn is_ci() -> bool {
    // Common CI environment variables
    env::var("CI").is_ok()
        || env::var("CONTINUOUS_INTEGRATION").is_ok()
        || env::var("GITHUB_ACTIONS").is_ok()
        || env::var("GITLAB_CI").is_ok()
        || env::var("CIRCLECI").is_ok()
        || env::var("TRAVIS").is_ok()
        || env::var("JENKINS_URL").is_ok()
        || env::var("BUILDKITE").is_ok()
        || env::var("TEAMCITY_VERSION").is_ok()
}

/// Flush pending telemetry with a timeout.
///
/// This function should be called before the application exits.
/// It waits for the pending telemetry task to complete, but will
/// timeout after 500ms to avoid delaying shutdown.
///
/// # Example
///
/// ```ignore
/// // At the end of main():
/// spawn::telemetry::flush().await;
/// ```
pub async fn flush() {
    let mutex = match PENDING_TASK.get() {
        Some(m) => m,
        None => return, // No telemetry was ever recorded
    };

    let handle = {
        let mut guard = mutex.lock().await;
        guard.take()
    };

    if let Some(handle) = handle {
        // Create a timeout future
        let timeout = tokio::time::timeout(Duration::from_millis(500), handle);

        match timeout.await {
            Ok(Ok(())) => {
                // Task completed successfully within timeout
            }
            Ok(Err(_)) => {
                // Task panicked - silently ignore
            }
            Err(_) => {
                // Timeout - task is still running, but we exit anyway
                // The task will be cancelled when the runtime shuts down
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Mutex to serialize tests that modify environment variables
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    #[test]
    fn test_do_not_track_disables_telemetry() {
        let _guard = ENV_MUTEX.lock().unwrap();
        env::set_var("DO_NOT_TRACK", "1");
        let recorder = TelemetryRecorder::new(Some("test-id"), true, "test".to_string(), vec![]);
        assert!(!recorder.enabled);
        env::remove_var("DO_NOT_TRACK");
    }

    #[test]
    fn test_telemetry_enabled_false_disables() {
        let _guard = ENV_MUTEX.lock().unwrap();
        env::remove_var("DO_NOT_TRACK"); // Ensure clean state
        let recorder = TelemetryRecorder::new(Some("test-id"), false, "test".to_string(), vec![]);
        assert!(!recorder.enabled);
    }

    #[test]
    fn test_uses_project_id_as_distinct_id() {
        let _guard = ENV_MUTEX.lock().unwrap();
        env::remove_var("DO_NOT_TRACK"); // Ensure clean state
        let recorder =
            TelemetryRecorder::new(Some("my-project-123"), true, "test".to_string(), vec![]);
        assert!(recorder.enabled);
        assert_eq!(recorder.distinct_id, "my-project-123");
    }

    #[test]
    fn test_generates_ephemeral_id_without_project_id() {
        let _guard = ENV_MUTEX.lock().unwrap();
        env::remove_var("DO_NOT_TRACK"); // Ensure clean state
        let recorder = TelemetryRecorder::new(None, true, "test".to_string(), vec![]);
        assert!(recorder.enabled);
        // Should be a valid UUID
        assert!(uuid::Uuid::parse_str(&recorder.distinct_id).is_ok());
    }

    #[test]
    fn test_properties_are_stored() {
        let _guard = ENV_MUTEX.lock().unwrap();
        env::remove_var("DO_NOT_TRACK"); // Ensure clean state
        let recorder = TelemetryRecorder::new(
            Some("test-id"),
            true,
            "migration build".to_string(),
            vec![("opt_pinned", "true".to_string())],
        );
        assert_eq!(recorder.command, "migration build");
        assert_eq!(recorder.properties.len(), 1);
        assert_eq!(
            recorder.properties[0],
            ("opt_pinned".to_string(), "true".to_string())
        );
    }
}
