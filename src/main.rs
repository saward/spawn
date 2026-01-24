use anyhow::Result;
use clap::Parser;
use opendal::services::Fs;
use opendal::Operator;
use spawn::cli::{run_cli, Cli};
use spawn::commands::{Outcome, TelemetryDescribe};
use spawn::telemetry::{self, CommandStatus, TelemetryRecorder};

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Handle internal telemetry child process (runs synchronously, no tokio runtime)
    if cli.internal_telemetry {
        telemetry::run_internal_telemetry();
        return Ok(());
    }

    // Run the async main for normal commands
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_main(cli))
}

async fn async_main(cli: Cli) -> Result<()> {
    let service = Fs::default().root(".");
    let config_fs = Operator::new(service)?.finish();

    // Get telemetry info from CLI before running
    let telemetry_info = cli.telemetry();

    // Start timing before command execution
    let start_time = std::time::Instant::now();

    // Run the CLI - this returns telemetry config along with outcome
    let result = run_cli(cli, &config_fs).await;

    // Create telemetry recorder with config from CLI result
    // Pass in the start time so duration is measured correctly
    let recorder = TelemetryRecorder::with_start_time(
        result.project_id.as_deref(),
        result.telemetry_enabled,
        telemetry_info,
        start_time,
    );

    // Finish telemetry based on outcome
    let (status, error_kind) = match &result.outcome {
        Ok(_) => (CommandStatus::Success, None),
        Err(e) => {
            let kind = extract_error_kind(e);
            (CommandStatus::Error, Some(kind))
        }
    };

    // This spawns a detached child process to send telemetry
    // The main process can exit immediately
    recorder.finish(status, error_kind.as_deref());

    // Handle the actual outcome
    match result.outcome? {
        Outcome::BuiltMigration { content } => {
            println!("{}", content);
        }
        Outcome::AppliedMigrations => {
            println!("All migrations applied successfully.");
        }
        Outcome::NewMigration(name) => {
            println!("New migration created: {}", name);
        }
        Outcome::NewTest(name) => {
            println!("New test created: {}", name);
        }
        Outcome::PinnedMigration { hash } => {
            println!("Migration pinned: {}", hash);
        }
        Outcome::Success => {
            println!("Success.");
        }
        Outcome::Unimplemented => {
            println!("Unimplemented command.");
        }
    }

    Ok(())
}

/// Extract a sanitized error kind from an anyhow::Error
fn extract_error_kind(error: &anyhow::Error) -> String {
    // Try to get the root cause type name
    let root = error.root_cause();
    let type_name = std::any::type_name_of_val(root);

    // Extract just the type name without the full path
    type_name
        .rsplit("::")
        .next()
        .unwrap_or("Unknown")
        .to_string()
}
