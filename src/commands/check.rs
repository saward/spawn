use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::migrator::Migrator;
use crate::store::list_migration_fs_status;
use anyhow::Result;
use console::style;

pub struct Check;

impl TelemetryDescribe for Check {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("check")
    }
}

impl Command for Check {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        // Validate the target reference if one was provided
        if config.target.is_some() {
            config.target_config()?;
        }

        let mut warnings: Vec<String> = Vec::new();

        // Grab status from store
        let fs_status = list_migration_fs_status(config.operator(), &config.pather(), None).await?;

        for (name, status) in &fs_status {
            if !status.has_up_sql {
                continue;
            }

            let pinned = if status.has_lock_toml {
                let migrator = Migrator::new(config, name, false);
                match migrator.load_migration_config().await {
                    Ok(cfg) => cfg.pin.is_some(),
                    // Its own warning, rather than a hard failure that would
                    // hide every other migration's.
                    Err(e) => {
                        warnings.push(format!(
                            "Migration {} has an unreadable lock.toml: {:#}",
                            style(name).yellow(),
                            e
                        ));
                        continue;
                    }
                }
            } else {
                false
            };
            if !pinned {
                warnings.push(format!("Migration {} is not pinned", style(name).yellow()));
            }
        }

        if warnings.is_empty() {
            println!("No issues found.");
            Ok(Outcome::Success)
        } else {
            println!(
                "\nFound {} warning{}:\n",
                warnings.len(),
                if warnings.len() == 1 { "" } else { "s" }
            );
            for (i, warning) in warnings.iter().enumerate() {
                println!("  {}. {}", i + 1, warning);
            }
            println!();
            Ok(Outcome::CheckFailed)
        }
    }
}
