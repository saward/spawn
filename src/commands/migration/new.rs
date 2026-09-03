use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::migrator::Migrator;
use anyhow::{Context, Result};

pub struct NewMigration {
    pub name: String,
}

impl TelemetryDescribe for NewMigration {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("migration new")
    }
}

impl Command for NewMigration {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let migration_name = format!(
            "{}-{}",
            chrono::Utc::now().format("%Y%m%d%H%M%S"),
            self.name
        );
        println!("creating migration with name {}", &migration_name);
        let mg = Migrator::new(config, &migration_name, false);

        let migration_template: Option<String> = match &config.migration_template {
            Some(t) => {
                let path = config.pather().any_path(t);
                let content = config
                    .operator()
                    .read(&path)
                    .await
                    .context(format!("Failed to migrations file '{}'", &path))?
                    .to_bytes();
                Some(
                    String::from_utf8(content.to_vec())
                        .context("Variables file is not valid UTF-8")?,
                )
            }
            None => None,
        };

        Ok(Outcome::NewMigration(
            mg.create_migration(migration_template).await?,
        ))
    }
}
