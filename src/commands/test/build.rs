use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::secrets::SecretsRenderMode;
use crate::sqltest::Tester;
use anyhow::Result;

pub struct BuildTest {
    pub name: String,
    /// Render real secret values instead of masked placeholders.
    pub reveal_secrets: bool,
}

impl TelemetryDescribe for BuildTest {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("test build").with_properties(vec![(
            "opt_reveal_secrets",
            self.reveal_secrets.to_string(),
        )])
    }
}

impl Command for BuildTest {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let tester = Tester::new(config, &self.name);
        let secrets_mode = if self.reveal_secrets {
            SecretsRenderMode::Revealed
        } else {
            SecretsRenderMode::Masked
        };
        let result = tester.generate(None, secrets_mode).await?;
        println!("{}", result);
        Ok(Outcome::Success)
    }
}
