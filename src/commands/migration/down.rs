use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::variables::Variables;
use anyhow::Result;

pub struct DownMigration {
    pub migration: String,
    pub variables: Option<Variables>,
    pub yes: bool,
    pub reuse_connection: bool,
}

impl TelemetryDescribe for DownMigration {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("migration down").with_properties(vec![
            ("has_variables", self.variables.is_some().to_string()),
            ("opt_reuse_connection", self.reuse_connection.to_string()),
        ])
    }
}

impl Command for DownMigration {
    async fn execute(&self, _config: &Config) -> Result<Outcome> {
        todo!("down implementation")
    }
}
