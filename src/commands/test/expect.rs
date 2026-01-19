use crate::commands::{Command, Outcome, TelemetryDescribe};
use crate::config::Config;
use crate::sqltest::Tester;
use anyhow::Result;

pub struct ExpectTest {
    pub name: String,
}

impl TelemetryDescribe for ExpectTest {
    fn telemetry_command(&self) -> String {
        "test expect".to_string()
    }
}

impl Command for ExpectTest {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let tester = Tester::new(config, &self.name);
        tester.save_expected(None).await?;
        Ok(Outcome::Success)
    }
}
