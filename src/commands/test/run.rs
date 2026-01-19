use crate::commands::{Command, Outcome, TelemetryDescribe};
use crate::config::Config;
use crate::sqltest::Tester;
use anyhow::Result;

pub struct RunTest {
    pub name: String,
}

impl TelemetryDescribe for RunTest {
    fn telemetry_command(&self) -> String {
        "test run".to_string()
    }
}

impl Command for RunTest {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let tester = Tester::new(config, &self.name);
        let result = tester.run(None).await?;
        println!("{}", result);
        Ok(Outcome::Success)
    }
}
