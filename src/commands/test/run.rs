use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::sqltest::Tester;
use anyhow::Result;
use futures::TryStreamExt;

pub struct RunTest {
    pub name: Option<String>,
}

impl TelemetryDescribe for RunTest {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("test run")
    }
}

impl Command for RunTest {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let test_names: Vec<String> = match &self.name {
            Some(name) => vec![name.clone()],
            None => {
                let mut tests: Vec<String> = Vec::new();
                let mut fs_lister = config
                    .operator()
                    .lister(&config.pather().tests_folder())
                    .await?;
                while let Some(entry) = fs_lister.try_next().await? {
                    let path = entry.path().to_string();
                    if path.ends_with("/") {
                        tests.push(path)
                    }
                }
                tests
            }
        };

        for test_name in test_names {
            let tester = Tester::new(config, &test_name);
            let result = tester.run(None).await?;
            println!("{}", result);
        }

        Ok(Outcome::Success)
    }
}
