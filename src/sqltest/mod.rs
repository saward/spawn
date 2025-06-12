use crate::config;
use crate::template;
use similar::DiffableStr;
use similar::{ChangeTag, TextDiff};
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::str;

use anyhow::{Context, Result};

pub struct Tester {
    config: config::Config,
    script_path: OsString,
}

#[derive(Debug)]
pub struct TestOutcome {
    matches: bool,
}

impl Tester {
    pub fn new(config: &config::Config, script_path: OsString) -> Self {
        Tester {
            config: config.clone(),
            script_path,
        }
    }

    pub fn components_folder(&self) -> PathBuf {
        self.config.spawn_folder.join("components")
    }

    pub fn tests_folder(&self) -> PathBuf {
        self.config.spawn_folder.join("tests")
    }

    pub fn test_folder(&self) -> PathBuf {
        self.tests_folder().join(self.script_path.clone())
    }

    pub fn script_file_path(&self) -> PathBuf {
        self.test_folder().join("test.sql")
    }

    pub fn expected_file_path(&self) -> PathBuf {
        self.test_folder().join("expected")
    }

    /// Opens the specified script file and generates a migration script, compiled
    /// using minijinja.
    pub fn generate(&self, variables: Option<crate::variables::Variables>) -> Result<String> {
        let lock_file = None;

        // Add our migration script to environment:
        let full_script_path = self.script_file_path();
        let contents = std::fs::read_to_string(&full_script_path).context(format!(
            "Failed to read test script '{}'",
            full_script_path.display()
        ))?;

        let gen = template::generate(&self.config, lock_file, &contents, variables)?;
        let content = gen.content;

        let mut parts = self.config.psql_command.clone();
        let command = parts.remove(0);
        let mut child = &mut Command::new(command);
        for arg in parts {
            child = child.arg(arg);
        }
        let mut child = child
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to execute command");

        let mut stdin = child.stdin.take().expect("failed to open stdin");
        std::thread::spawn(move || {
            stdin
                .write(&content.as_bytes())
                .expect("failed to write to stdin");
        });

        let result = child.wait_with_output()?;
        if !result.status.success() {
            eprintln!("psql exited with status {}", result.status);
        }

        let out: String = str::from_utf8(&result.stdout)?.to_string();

        Ok(out)
    }

    // Runs the test and compares the actual output to expected.
    pub fn run(&self, variables: Option<crate::variables::Variables>) -> Result<TestOutcome> {
        let expected = fs::read_to_string(self.expected_file_path())
            .context("unable to read expectations file")?;

        let content = self
            .generate(variables)
            .context("could not generate test script")?;

        let matches = match self.compare(&content, &expected) {
            Ok(()) => true,
            Err(differences) => {
                println!("Differences found:\n{}", differences);
                false
            }
        };

        let outcome = TestOutcome { matches };
        Ok(outcome)
    }

    pub fn save_expected(&self, variables: Option<crate::variables::Variables>) -> Result<()> {
        let content = self.generate(variables)?;
        fs::write(self.expected_file_path(), content)
            .context("unable to write expectation file")?;

        Ok(())
    }

    pub fn compare(&self, generated: &str, expected: &str) -> std::result::Result<(), String> {
        let diff = dissimilar::diff(generated, expected);

        let mut differences = String::new();
        let mut generated_line = 1;
        let mut diff_found = false;

        for chunk in diff {
            match chunk {
                dissimilar::Chunk::Equal(s) => {
                    Self::append_lines(&mut differences, " ", &mut generated_line, s, true);
                }
                dissimilar::Chunk::Delete(s) => {
                    diff_found = true;
                    Self::append_lines(&mut differences, "-", &mut generated_line, s, false);
                }
                dissimilar::Chunk::Insert(s) => {
                    diff_found = true;
                    Self::append_lines(&mut differences, "+", &mut generated_line, s, true);
                }
            }
        }

        if !diff_found {
            return Ok(());
        }

        Err(differences)
    }

    fn append_lines(
        differences: &mut String,
        prefix: &str,
        start_line: &mut usize,
        s: &str,
        count_lines: bool,
    ) {
        for line in s.split_inclusive('\n') {
            differences.push_str(&format!("{} {: >6}: {}", prefix, *start_line, line));

            // Ensure newline at end if missing
            if !line.ends_with('\n') {
                differences.push('\n');
            }

            if count_lines {
                *start_line += line.matches('\n').count();
            }
        }
    }
}
