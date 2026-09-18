use crate::commands::migration::{get_combined_migration_status, PinMatchStatus};
use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::engine::MigrationStatus as EngineStatus;
use anyhow::Result;
use console::style;
use tabled::settings::Style;
use tabled::{Table, Tabled};

#[derive(Tabled)]
struct MigrationStatusDisplay {
    #[tabled(rename = "Migration")]
    name: String,
    #[tabled(rename = "Filesystem")]
    on_filesystem: String,
    #[tabled(rename = "Pinned")]
    pinned: String,
    #[tabled(rename = "Pin Match")]
    pin_match: String,
    #[tabled(rename = "Database")]
    in_database: String,
    #[tabled(rename = "Status")]
    status: String,
    #[tabled(rename = "Checksum Match")]
    checksum_match: String,
}

pub struct MigrationStatus;

impl TelemetryDescribe for MigrationStatus {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("migration status")
    }
}

impl Command for MigrationStatus {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let status_rows =
            get_combined_migration_status(config, Some(super::DEFAULT_NAMESPACE)).await?;

        if status_rows.is_empty() {
            println!("No migrations found");
            return Ok(Outcome::Success);
        }

        let display_rows: Vec<MigrationStatusDisplay> = status_rows
            .into_iter()
            .map(|row| {
                let on_filesystem = if row.exists_in_filesystem {
                    style("✓").green().to_string()
                } else {
                    style("✗").red().to_string()
                };

                let pinned = if row.is_pinned {
                    style("✓").green().to_string()
                } else {
                    style("✗").red().to_string()
                };

                let pin_match = match row.pin_matches {
                    PinMatchStatus::Matches => style("✓").green().to_string(),
                    PinMatchStatus::Differs => style("✗").red().to_string(),
                    PinMatchStatus::Missing => style("?").yellow().to_string(),
                    PinMatchStatus::NotApplicable => style("-").dim().to_string(),
                };

                let checksum_match = match row.checksum_matches {
                    Some(true) => style("✓").green().to_string(),
                    Some(false) => style("✗").red().to_string(),
                    None => style("-").dim().to_string(),
                };

                let in_database = if row.exists_in_db {
                    style("✓").green().to_string()
                } else {
                    style("✗").red().to_string()
                };

                // Determine status display based on whether it's applied and how
                let status = match (
                    row.exists_in_db,
                    row.last_status,
                    row.last_activity.as_deref(),
                ) {
                    (true, Some(EngineStatus::Success), Some("APPLY")) => {
                        style("✓ Applied").green().to_string()
                    }
                    (true, Some(EngineStatus::Success), Some("ADOPT")) => {
                        style("⊙ Adopted").cyan().to_string()
                    }
                    (true, Some(EngineStatus::Attempted), _) => {
                        style("⚠ Attempted").yellow().to_string()
                    }
                    (true, Some(EngineStatus::Failure), _) => style("✗ Failed").red().to_string(),
                    (false, _, _) => style("○ Pending").dim().to_string(),
                    _ => style("-").dim().to_string(),
                };

                MigrationStatusDisplay {
                    name: row.migration_name,
                    on_filesystem,
                    pinned,
                    pin_match,
                    in_database,
                    status,
                    checksum_match,
                }
            })
            .collect();

        let mut table = Table::new(display_rows);
        table.with(Style::sharp());
        println!("\n{}\n", table);

        Ok(Outcome::Success)
    }
}
