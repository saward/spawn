use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::sqltest::Tester;
use anyhow::Result;

pub struct NewTest {
    pub name: String,
}

impl TelemetryDescribe for NewTest {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("test new")
    }
}

impl Command for NewTest {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        println!("creating test with name {}", &self.name);
        let tester = Tester::new(config, &self.name);

        Ok(Outcome::NewTest(tester.create_test().await?))
    }
}
