use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::sqltest::Tester;
use anyhow::Result;

pub struct ExpectTest {
    pub name: String,
}

impl TelemetryDescribe for ExpectTest {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("test expect")
    }
}

impl Command for ExpectTest {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let tester = Tester::new(config, &self.name);
        tester.save_expected(None).await?;
        Ok(Outcome::Success)
    }
}
