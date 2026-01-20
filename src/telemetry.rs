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
use posthog_rs::{ClientOptions, ClientOptionsBuilder, Event};
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
    /// Create a new telemetry recorder.
    ///
    /// Checks opt-out settings in priority order:
    /// 1. `DO_NOT_TRACK` env var -> Disable
    /// 2. `telemetry_enabled = false` -> Disable
    /// 3. Otherwise -> Enable
    ///
    /// If no `project_id` is provided, generates an ephemeral UUID for this session.
    pub fn new(project_id: Option<&str>, telemetry_enabled: bool, info: TelemetryInfo) -> Self {
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
            start_time: Instant::now(),
        }
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
            "spawning child for event: command={}, distinct_id={}",
            event.command,
            event.distinct_id
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

/// Send the telemetry event to PostHog
async fn send_event(
    distinct_id: &str,
    command: &str,
    duration_ms: u64,
    status: CommandStatus,
    error_kind: Option<&str>,
    properties: Vec<(String, String)>,
) -> anyhow::Result<()> {
    debug_telemetry!(
        "send_event called: distinct_id={}, command={}, duration_ms={}",
        distinct_id,
        command,
        duration_ms
    );

    let options: ClientOptions = ClientOptionsBuilder::default()
        .api_endpoint("https://eu.i.posthog.com/capture/".to_string())
        .api_key(POSTHOG_API_KEY.to_string())
        .build()?;

    match posthog_rs::init_global(options).await {
        Ok(_) => debug_telemetry!("PostHog client initialized"),
        Err(posthog_rs::Error::AlreadyInitialized) => {
            debug_telemetry!("PostHog client already initialized")
        }
        Err(e) => debug_telemetry!("PostHog init error: {:?}", e),
    }

    let mut event = Event::new("command_completed", distinct_id);

    // Helper closure to convert PostHog errors to anyhow errors
    let to_anyhow = |e| anyhow::anyhow!("PostHog error: {:?}", e);

    event
        .insert_prop("app_version", APP_VERSION)
        .map_err(to_anyhow)?;
    event
        .insert_prop("os_platform", std::env::consts::OS)
        .map_err(to_anyhow)?;
    event
        .insert_prop("os_arch", std::env::consts::ARCH)
        .map_err(to_anyhow)?;
    event.insert_prop("is_ci", is_ci()).map_err(to_anyhow)?;

    // Usage properties
    event.insert_prop("command", command).map_err(to_anyhow)?;
    event
        .insert_prop("duration_ms", duration_ms)
        .map_err(to_anyhow)?;

    // Command-specific properties
    for (key, value) in properties {
        event.insert_prop(key, value).map_err(to_anyhow)?;
    }

    // Health properties
    event
        .insert_prop("status", status.to_string())
        .map_err(to_anyhow)?;
    if let Some(kind) = error_kind {
        event.insert_prop("error_kind", kind).map_err(to_anyhow)?;
    }

    debug_telemetry!("calling posthog_rs::capture...");

    // Capture also returns posthog_rs::Error, so we map it here too
    posthog_rs::capture(event).await.map_err(to_anyhow)?;

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

    // Create a minimal tokio runtime just for the HTTP calls
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

    // Send each event
    rt.block_on(async {
        for event in events {
            debug_telemetry!(
                "sending event: command={}, distinct_id={}",
                event.command,
                event.distinct_id
            );

            let result = send_event(
                &event.distinct_id,
                &event.command,
                event.duration_ms,
                event.status,
                event.error_kind.as_deref(),
                event.properties,
            )
            .await;

            match result {
                Ok(()) => debug_telemetry!("sent event: {}", event.command),
                Err(e) => debug_telemetry!("failed to send event {}: {:?}", event.command, e),
            }
        }
    });

    debug_telemetry!("child finished sending events");
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
