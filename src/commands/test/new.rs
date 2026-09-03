use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::sqltest::Tester;
use anyhow::{Context, Result};

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

        let test_template: Option<String> = match &config.test_template {
            Some(t) => {
                let path = config.pather().any_path(t);
                let content = config
                    .operator()
                    .read(&path)
                    .await
                    .context(format!("Failed to read test template file '{}'", &path))?
                    .to_bytes();
                Some(
                    String::from_utf8(content.to_vec())
                        .context("Test template file is not valid UTF-8")?,
                )
            }
            None => None,
        };

        Ok(Outcome::NewTest(tester.create_test(test_template).await?))
    }
}
