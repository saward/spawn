use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::sqltest::Tester;
use anyhow::Result;
use futures::TryStreamExt;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";

pub struct CompareTests {
    pub name: Option<String>,
}

impl TelemetryDescribe for CompareTests {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("test compare")
            .with_properties(vec![("is_comparing_all", self.name.is_none().to_string())])
    }
}

impl Command for CompareTests {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let test_files: Vec<String> = match &self.name {
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

        let mut failed = false;

        for test_file in test_files {
            let tester = Tester::new(config, &test_file);

            match tester.run_compare(None).await {
                Ok(result) => match result.diff {
                    None => {
                        println!("{}[PASS]{} {}", GREEN, RESET, test_file);
                    }
                    Some(diff) => {
                        failed = true;
                        println!("\n{}[FAIL]{} {}{}{}", RED, RESET, BOLD, test_file, RESET);
                        println!("{}--- Diff ---{}", BOLD, RESET);
                        println!("{}", diff);
                        println!("{}-------------{}\n", BOLD, RESET);
                    }
                },
                Err(e) => return Err(e),
            };
        }

        if failed {
            return Err(anyhow::anyhow!(
                "{}!{} Differences found in one or more tests",
                RED,
                RESET
            ));
        }

        Ok(Outcome::Success)
    }
}
