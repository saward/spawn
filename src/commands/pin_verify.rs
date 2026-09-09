use anyhow::Result;

use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::store::pinner::spawn::verify_store;

pub struct PinVerify;

impl TelemetryDescribe for PinVerify {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("verify")
    }
}

impl Command for PinVerify {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let pather = config.pather();
        let fs = config.operator();

        let result = verify_store(fs, &pather).await?;

        if result.corrupted.is_empty() {
            println!(
                "Checked {} pinned file(s): all match their content hash.",
                result.checked_count
            );
        } else {
            println!(
                "Checked {} pinned file(s): {} corrupted.",
                result.checked_count,
                result.corrupted.len()
            );
            for file in &result.corrupted {
                println!(
                    "  {} — expected hash '{}', got '{}'",
                    file.path, file.expected_hash, file.actual_hash
                );
            }
        }

        if result.missing_roots.is_empty() {
            println!("All migration lock files point to a root hash that exists in the store.");
        } else {
            println!(
                "{} migration(s) point to a missing root hash:",
                result.missing_roots.len()
            );
            for missing in &result.missing_roots {
                println!(
                    "  {} — missing root hash '{}'",
                    missing.migration, missing.hash
                );
            }
        }

        let corrupted_count = result.corrupted.len();
        let missing_root_count = result.missing_roots.len();

        Ok(Outcome::PinVerify {
            corrupted_count,
            missing_root_count,
        })
    }
}
