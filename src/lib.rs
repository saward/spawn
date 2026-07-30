pub mod cli;
pub mod commands;
pub mod completions;
pub mod config;
pub mod engine;
pub mod escape;
pub mod migrator;
pub mod pinfile;
pub mod sql_formatter;
pub mod sqltest;
pub mod store;
pub mod telemetry;
pub mod template;
pub mod variables;

/// Display the telemetry notice to stderr
pub fn show_telemetry_notice() {
    eprintln!("▶ Spawn collects anonymous usage data.");
    eprintln!("  This helps us improve Spawn.");
    eprintln!(
        "  Set \"telemetry = false\" in {} or use DO_NOT_TRACK=1 to opt-out.",
        config::DEFAULT_CONFIG_FILE
    );
    eprintln!();
}
