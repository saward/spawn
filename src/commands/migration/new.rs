use crate::commands::{Command, Outcome, TelemetryDescribe};
use crate::config::Config;
use crate::migrator::Migrator;
use anyhow::Result;

pub struct NewMigration {
    pub name: String,
}

impl TelemetryDescribe for NewMigration {
    fn telemetry_command(&self) -> String {
        "migration new".to_string()
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

        Ok(Outcome::NewMigration(mg.create_migration().await?))
    }
}
