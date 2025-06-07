use crate::config;
use crate::template;
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

pub struct Tester {
    config: config::Config,
    script_path: OsString,
}

impl Tester {
    pub fn new(config: &config::Config, script_path: OsString) -> Self {
        Tester {
            config: config.clone(),
            script_path,
        }
    }

    pub fn components_folder(&self) -> PathBuf {
        self.config.scripts_path.join("components")
    }

    pub fn tests_folder(&self) -> PathBuf {
        self.config.scripts_path.join("tests")
    }

    pub fn script_file_path(&self) -> PathBuf {
        self.tests_folder()
            .join(self.script_path.clone())
            .join("test.sql")
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
            .spawn()
            .expect("Failed to execute command");

        let mut stdin = child.stdin.take().expect("Failed to open stdin");
        std::thread::spawn(move || {
            stdin
                .write(&content.as_bytes())
                .expect("Failed to write to stdin");
        });

        let status = child.wait()?;
        if !status.success() {
            eprintln!("psql exited with status {}", status);
        }

        Ok("".to_string())
    }
}
