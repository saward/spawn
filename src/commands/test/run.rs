use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::sqltest::Tester;
use anyhow::Result;

pub struct RunTest {
    pub name: String,
}

impl TelemetryDescribe for RunTest {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("test run")
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
