use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::sqltest::Tester;
use anyhow::Result;

pub struct BuildTest {
    pub name: String,
}

impl TelemetryDescribe for BuildTest {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("test build")
    }
}

impl Command for BuildTest {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let tester = Tester::new(config, &self.name);
        let result = tester.generate(None).await?;
        println!("{}", result);
        Ok(Outcome::Success)
    }
}
