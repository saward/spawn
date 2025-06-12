use crate::config;
use crate::template;
use console::{style, Style};
use similar::{ChangeTag, TextDiff};
use std::ffi::OsString;
use std::fmt;
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
    pub diff: Option<String>,
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

        let outcome = match self.compare(&content, &expected) {
            Ok(()) => TestOutcome { diff: None },
            Err(differences) => TestOutcome {
                diff: Some(differences.to_string()),
            },
        };

        return Ok(outcome);
    }

    pub fn save_expected(&self, variables: Option<crate::variables::Variables>) -> Result<()> {
        let content = self.generate(variables)?;
        fs::write(self.expected_file_path(), content)
            .context("unable to write expectation file")?;

        Ok(())
    }

    pub fn compare(&self, generated: &str, expected: &str) -> std::result::Result<(), String> {
        let diff = TextDiff::from_lines(generated, expected);

        let mut diff_display = String::new();

        for (idx, group) in diff.grouped_ops(3).iter().enumerate() {
            if idx > 0 {
                diff_display.push_str(&format!("{:-^1$}", "-", 80));
            }
            for op in group {
                for change in diff.iter_inline_changes(op) {
                    let (sign, s) = match change.tag() {
                        ChangeTag::Delete => ("-", Style::new().red()),
                        ChangeTag::Insert => ("+", Style::new().green()),
                        ChangeTag::Equal => (" ", Style::new().dim()),
                    };
                    diff_display.push_str(&format!(
                        "{}{} |{}",
                        style(Line(change.old_index())).dim(),
                        style(Line(change.new_index())).dim(),
                        s.apply_to(sign).bold(),
                    ));
                    for (emphasized, value) in change.iter_strings_lossy() {
                        if emphasized {
                            diff_display.push_str(&format!(
                                "{}",
                                s.apply_to(value).underlined().on_black()
                            ));
                        } else {
                            diff_display.push_str(&format!("{}", s.apply_to(value)));
                        }
                    }
                    if change.missing_newline() {
                        diff_display.push('\n');
                    }
                }
            }
        }

        if diff_display.len() > 0 {
            return Err(diff_display);
        }

        Ok(())
    }
}

struct Line(Option<usize>);

impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.0 {
            None => write!(f, "    "),
            Some(idx) => write!(f, "{:<4}", idx + 1),
        }
    }
}
