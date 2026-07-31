use anyhow::Result;

use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::store::pinner::gc::gc_pinned;

pub struct PinCleanup {
    pub dry_run: bool,
}

impl TelemetryDescribe for PinCleanup {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("cleanup").with_properties(vec![("dry_run", self.dry_run.to_string())])
    }
}

impl Command for PinCleanup {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let pather = config.pather();
        let fs = config.operator();

        let result = gc_pinned(fs, &pather, self.dry_run).await?;

        if self.dry_run {
            if result.orphaned.is_empty() {
                println!("No orphaned pinned files found.");
            } else {
                println!(
                    "Would delete {} orphaned pinned file(s):",
                    result.orphaned.len()
                );
                for hash in &result.orphaned {
                    println!("  {}", hash);
                }
            }
            println!(
                "\n{} pinned file(s) are still referenced by migrations.",
                result.referenced_count
            );
            println!("\nRun without --dry-run to delete orphaned files.");
        } else if result.orphaned.is_empty() {
            println!("No orphaned pinned files found. Nothing to clean up.");
        } else {
            println!("Deleted {} orphaned pinned file(s).", result.orphaned.len());
            println!(
                "{} pinned file(s) are still referenced by migrations.",
                result.referenced_count
            );
        }

        Ok(Outcome::PinCleanup {
            orphaned: result.orphaned,
            referenced_count: result.referenced_count,
            dry_run: result.dry_run,
        })
    }
}
