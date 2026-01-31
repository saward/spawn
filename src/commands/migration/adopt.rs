use crate::commands::migration::get_pending_and_confirm;
use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::engine::MigrationError;
use anyhow::{anyhow, Result};
use dialoguer::Editor;

pub struct AdoptMigration {
    pub migration: Option<String>,
    pub yes: bool,
    pub description: Option<String>,
}

impl TelemetryDescribe for AdoptMigration {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("migration adopt")
    }
}

/// Prompt the user for a description using their preferred editor.
/// Returns an error if the description is empty or the editor is aborted.
fn prompt_description() -> Result<String> {
    let description = Editor::new()
        .require_save(true)
        .edit("# Why is this migration being adopted?\n# Lines starting with # will be ignored.\n")?
        .map(|s| {
            s.lines()
                .filter(|line| !line.starts_with('#'))
                .collect::<Vec<_>>()
                .join("\n")
                .trim()
                .to_string()
        })
        .unwrap_or_default();

    if description.is_empty() {
        return Err(anyhow!("A description is required when adopting migrations. Use --description or provide one in the editor."));
    }

    Ok(description)
}

impl Command for AdoptMigration {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let migrations = match &self.migration {
            Some(migration) => vec![migration.clone()],
            None => match get_pending_and_confirm(config, "adopt", self.yes).await? {
                Some(pending) => pending,
                None => return Ok(Outcome::AdoptedMigration),
            },
        };

        let description = match &self.description {
            Some(desc) => {
                if desc.trim().is_empty() {
                    return Err(anyhow!(
                        "A description is required when adopting migrations."
                    ));
                }
                desc.clone()
            }
            None => prompt_description()?,
        };

        let engine = config.new_engine().await?;

        let total = migrations.len();
        for (i, migration) in migrations.iter().enumerate() {
            let counter = if total > 1 {
                format!(
                    "[{:>width$}/{}] ",
                    i + 1,
                    total,
                    width = total.to_string().len()
                )
            } else {
                String::new()
            };
            match engine
                .migration_adopt(migration, super::DEFAULT_NAMESPACE, &description)
                .await
            {
                Ok(msg) => {
                    println!("{}{}", counter, msg);
                }
                Err(MigrationError::AlreadyApplied { info, .. }) => {
                    println!(
                        "{}Migration '{}' already applied (status: {}, activity: {})",
                        counter, migration, info.last_status, info.last_activity
                    );
                }
                Err(e) => {
                    return Err(
                        anyhow!(e).context(format!("Failed adopting migration '{}'", migration))
                    );
                }
            }
        }

        Ok(Outcome::AdoptedMigration)
    }
}
