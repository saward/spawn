use anyhow::Result;
use clap::Parser;
use opendal::services::Fs;
use opendal::Operator;
use spawn::cli::{run_cli, Cli};
use spawn::commands::{Outcome, TelemetryDescribe};
use spawn::telemetry::{self, CommandStatus, TelemetryRecorder};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let service = Fs::default().root(".");
    let config_fs = Operator::new(service)?.finish();

    // Get telemetry info from CLI before running
    let telemetry_info = cli.telemetry();

    // Run the CLI - this returns telemetry config along with outcome
    let result = run_cli(cli, &config_fs).await;

    // Create telemetry recorder with config from CLI result
    let recorder = TelemetryRecorder::new(
        result.project_id.as_deref(),
        result.telemetry_enabled,
        telemetry_info,
    );

    // Finish telemetry based on outcome
    let (status, error_kind) = match &result.outcome {
        Ok(_) => (CommandStatus::Success, None),
        Err(e) => {
            let kind = extract_error_kind(e);
            (CommandStatus::Error, Some(kind))
        }
    };

    recorder.finish(status, error_kind.as_deref());

    // Wait for telemetry to send (with timeout)
    telemetry::flush().await;

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
