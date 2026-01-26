//! Telemetry module for anonymous usage data collection.
//!
//! This module collects anonymous usage statistics to help improve spawn.
//! It is designed to be:
//! - **Non-blocking**: Telemetry is sent by a detached child process
//! - **Privacy-respecting**: No personal data is collected
//! - **Fail-silent**: Errors are silently ignored
//!
//! ## Opt-out
//!
//! Telemetry can be disabled by:
//! 1. Setting the `DO_NOT_TRACK` environment variable (any value)
//! 2. Setting `telemetry = false` in `spawn.toml`
//!
//! ## Debugging
//!
//! Set `SPAWN_DEBUG_TELEMETRY=1` to enable debug output for telemetry.

use crate::commands::TelemetryInfo;
use serde::{Deserialize, Serialize};
use std::env;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::Instant;

/// Check if telemetry debug mode is enabled
fn debug_enabled() -> bool {
    env::var("SPAWN_DEBUG_TELEMETRY").is_ok()
}

/// Print debug message if SPAWN_DEBUG_TELEMETRY is set
macro_rules! debug_telemetry {
    ($($arg:tt)*) => {
        if debug_enabled() {
            eprintln!("[telemetry] {}", format!($($arg)*));
        }
    };
}

/// PostHog API key for spawn telemetry
const POSTHOG_API_KEY: &str = "phc_yD13QBdCJSnbIjmkTcSf03dRhpLJdCMfTVRzD7XTFqd";

/// PostHog API endpoint (EU Cloud)
const POSTHOG_ENDPOINT: &str = "https://eu.i.posthog.com/batch/";

/// Application version from Cargo.toml
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A telemetry event to be sent (serializable for IPC)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryEvent {
    pub distinct_id: String,
    pub command: String,
    pub duration_ms: u64,
    pub status: CommandStatus,
    pub error_kind: Option<String>,
    pub properties: Vec<(String, String)>,
}

/// Spawn a detached child process to send telemetry events
fn spawn_telemetry_child(events: &[TelemetryEvent]) {
    if events.is_empty() {
        return;
    }

    // Get the current executable path
    let exe_path = match env::current_exe() {
        Ok(path) => path,
        Err(e) => {
            debug_telemetry!("failed to get current exe: {:?}", e);
            return;
        }
    };

    // Serialize the events to JSON
    let json = match serde_json::to_string(events) {
        Ok(j) => j,
        Err(e) => {
            debug_telemetry!("failed to serialize events: {:?}", e);
            return;
        }
    };

    // Build the command
    let mut cmd = Command::new(&exe_path);
    cmd.arg("--internal-telemetry")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Platform-specific detachment
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS (0x8) | CREATE_NO_WINDOW (0x08000000)
        cmd.creation_flags(0x08000008);
    }

    // Spawn the child
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            debug_telemetry!("failed to spawn telemetry child: {:?}", e);
            return;
        }
    };

    // Write JSON to stdin
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(json.as_bytes()) {
            debug_telemetry!("failed to write to child stdin: {:?}", e);
        }
        // stdin is dropped here, closing the pipe
    }

    debug_telemetry!(
        "spawned telemetry child process for {} event(s)",
        events.len()
    );
    // Do NOT call child.wait() - let it run independently
}

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
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
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
    /// Create a new telemetry recorder with a pre-recorded start time.
    ///
    /// Use this when you need to start timing before you have all the
    /// telemetry configuration (e.g., before loading config from disk).
    ///
    /// Checks opt-out settings in priority order:
    /// 1. `DO_NOT_TRACK` env var -> Disable
    /// 2. `telemetry_enabled = false` -> Disable
    /// 3. Otherwise -> Enable
    ///
    /// If no `project_id` is provided, generates an ephemeral UUID for this session.
    pub fn with_start_time(
        project_id: Option<&str>,
        telemetry_enabled: bool,
        info: TelemetryInfo,
        start_time: Instant,
    ) -> Self {
        // Check DO_NOT_TRACK env var first (highest priority)
        let do_not_track = env::var("DO_NOT_TRACK").is_ok();

        // Determine if telemetry is enabled
        let enabled = !do_not_track && telemetry_enabled;

        // Get or generate distinct_id
        // Ephemeral IDs are prefixed with "e-" to distinguish them in analytics
        let distinct_id = if enabled {
            project_id
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("e-{}", uuid::Uuid::new_v4()))
        } else {
            String::new()
        };

        // Convert properties to owned strings
        let properties = info
            .properties
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();

        Self {
            enabled,
            distinct_id,
            command: info.label,
            properties,
            start_time,
        }
    }

    /// Create a new telemetry recorder, starting the timer now.
    ///
    /// Checks opt-out settings in priority order:
    /// 1. `DO_NOT_TRACK` env var -> Disable
    /// 2. `telemetry_enabled = false` -> Disable
    /// 3. Otherwise -> Enable
    ///
    /// If no `project_id` is provided, generates an ephemeral UUID for this session.
    pub fn new(project_id: Option<&str>, telemetry_enabled: bool, info: TelemetryInfo) -> Self {
        Self::with_start_time(project_id, telemetry_enabled, info, Instant::now())
    }

    /// Finish recording and spawn a detached child process to send telemetry.
    ///
    /// This method consumes the recorder and spawns a background process.
    /// The main process can exit immediately without waiting.
    pub fn finish(self, status: CommandStatus, error_kind: Option<&str>) {
        debug_telemetry!("finish() called, enabled={}", self.enabled);
        if !self.enabled {
            return;
        }

        let event = TelemetryEvent {
            distinct_id: self.distinct_id,
            command: self.command,
            duration_ms: self.start_time.elapsed().as_millis() as u64,
            status,
            error_kind: error_kind.map(|s| s.to_string()),
            properties: self.properties,
        };

        debug_telemetry!(
            "spawning child for event: command={}, distinct_id={}, duration_ms={}",
            event.command,
            event.distinct_id,
            event.duration_ms
        );

        // Spawn detached child process to send the event
        spawn_telemetry_child(&[event]);
    }
}

/// Send multiple telemetry events via a detached child process.
///
/// This is useful when you have collected multiple events and want to
/// send them all in a single child process.
pub fn send_events(events: Vec<TelemetryEvent>) {
    if events.is_empty() {
        return;
    }
    debug_telemetry!("sending {} event(s)", events.len());
    spawn_telemetry_child(&events);
}

/// Send telemetry events to PostHog using the batch API
async fn send_events_to_posthog(events: &[TelemetryEvent]) -> Result<(), reqwest::Error> {
    // Build batch payload
    let batch: Vec<serde_json::Value> = events
        .iter()
        .map(|event| {
            let mut props = serde_json::Map::new();
            props.insert(
                "distinct_id".to_string(),
                serde_json::json!(event.distinct_id),
            );
            props.insert("app_version".to_string(), serde_json::json!(APP_VERSION));
            props.insert(
                "os_platform".to_string(),
                serde_json::json!(std::env::consts::OS),
            );
            props.insert(
                "os_arch".to_string(),
                serde_json::json!(std::env::consts::ARCH),
            );
            props.insert("is_ci".to_string(), serde_json::json!(is_ci()));
            props.insert("command".to_string(), serde_json::json!(event.command));
            props.insert(
                "duration_ms".to_string(),
                serde_json::json!(event.duration_ms),
            );
            props.insert(
                "status".to_string(),
                serde_json::json!(event.status.to_string()),
            );
            props.insert("$lib".to_string(), serde_json::json!("spawn"));
            props.insert("$lib_version".to_string(), serde_json::json!(APP_VERSION));
            // Don't create person profiles for CLI telemetry
            props.insert(
                "$process_person_profile".to_string(),
                serde_json::json!(false),
            );

            if let Some(ref kind) = event.error_kind {
                props.insert("error_kind".to_string(), serde_json::json!(kind));
            }

            for (key, value) in &event.properties {
                props.insert(key.clone(), serde_json::json!(value));
            }

            serde_json::json!({
                "event": "command_completed",
                "properties": props
            })
        })
        .collect();

    let payload = serde_json::json!({
        "api_key": POSTHOG_API_KEY,
        "batch": batch
    });

    debug_telemetry!("POST to {}", POSTHOG_ENDPOINT);
    debug_telemetry!(
        "payload: {}",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
    );

    let client = reqwest::Client::new();
    let response = client
        .post(POSTHOG_ENDPOINT)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
        .await?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    debug_telemetry!("response status: {}, body: {}", status, body);

    Ok(())
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

/// Run the internal telemetry handler (called by child process).
///
/// This reads JSON-serialized TelemetryEvents from stdin and sends them to PostHog.
/// This function is meant to be called when the binary is invoked with `--internal-telemetry`.
pub fn run_internal_telemetry() {
    // Read JSON from stdin
    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        debug_telemetry!("failed to read stdin: {:?}", e);
        return;
    }

    // Parse the events
    let events: Vec<TelemetryEvent> = match serde_json::from_str(&input) {
        Ok(e) => e,
        Err(e) => {
            debug_telemetry!("failed to parse events JSON: {:?}", e);
            return;
        }
    };

    if events.is_empty() {
        debug_telemetry!("no events to send");
        return;
    }

    debug_telemetry!("child received {} event(s)", events.len());

    // Create a minimal tokio runtime just for the HTTP call
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            debug_telemetry!("failed to create runtime: {:?}", e);
            return;
        }
    };

    // Send all events in a single batch request
    let result = rt.block_on(send_events_to_posthog(&events));

    match result {
        Ok(()) => debug_telemetry!("successfully sent {} event(s)", events.len()),
        Err(e) => debug_telemetry!("failed to send events: {:?}", e),
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
        let recorder = TelemetryRecorder::new(Some("test-id"), true, TelemetryInfo::new("test"));
        assert!(!recorder.enabled);
        env::remove_var("DO_NOT_TRACK");
    }

    #[test]
    fn test_telemetry_enabled_false_disables() {
        let _guard = ENV_MUTEX.lock().unwrap();
        env::remove_var("DO_NOT_TRACK"); // Ensure clean state
        let recorder = TelemetryRecorder::new(Some("test-id"), false, TelemetryInfo::new("test"));
        assert!(!recorder.enabled);
    }

    #[test]
    fn test_uses_project_id_as_distinct_id() {
        let _guard = ENV_MUTEX.lock().unwrap();
        env::remove_var("DO_NOT_TRACK"); // Ensure clean state
        let recorder =
            TelemetryRecorder::new(Some("my-project-123"), true, TelemetryInfo::new("test"));
        assert!(recorder.enabled);
        assert_eq!(recorder.distinct_id, "my-project-123");
    }

    #[test]
    fn test_generates_ephemeral_id_without_project_id() {
        let _guard = ENV_MUTEX.lock().unwrap();
        env::remove_var("DO_NOT_TRACK"); // Ensure clean state
        let recorder = TelemetryRecorder::new(None, true, TelemetryInfo::new("test"));
        assert!(recorder.enabled);
        // Should be prefixed with "e-" and contain a valid UUID
        assert!(recorder.distinct_id.starts_with("e-"));
        let uuid_part = recorder.distinct_id.strip_prefix("e-").unwrap();
        assert!(uuid::Uuid::parse_str(uuid_part).is_ok());
    }

    #[test]
    fn test_properties_are_stored() {
        let _guard = ENV_MUTEX.lock().unwrap();
        env::remove_var("DO_NOT_TRACK"); // Ensure clean state
        let recorder = TelemetryRecorder::new(
            Some("test-id"),
            true,
            TelemetryInfo::new("migration build")
                .with_properties(vec![("opt_pinned", "true".to_string())]),
        );
        assert_eq!(recorder.command, "migration build");
        assert_eq!(recorder.properties.len(), 1);
        assert_eq!(
            recorder.properties[0],
            ("opt_pinned".to_string(), "true".to_string())
        );
    }
}
