use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::migrator::Migrator;
use crate::variables::Variables;
use anyhow::Result;

pub struct BuildMigration {
    pub migration: String,
    pub pinned: bool,
    pub variables: Option<Variables>,
}

impl TelemetryDescribe for BuildMigration {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("migration build").with_properties(vec![
            ("opt_pinned", self.pinned.to_string()),
            ("has_variables", self.variables.is_some().to_string()),
        ])
    }
}

impl Command for BuildMigration {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let mgrtr = Migrator::new(config, &self.migration, self.pinned);
        match mgrtr.generate_streaming(self.variables.clone()).await {
            Ok(gen) => {
                let mut buffer = Vec::new();
                gen.render_to_writer(&mut buffer)
                    .map_err(std::io::Error::other)?;
                let content = String::from_utf8(buffer)?;
                Ok(Outcome::BuiltMigration { content })
            }
            Err(e) => Err(e),
        }
    }
}
