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
        // docker exec -i spawn-db psql -U spawn spawn
        let mut child = Command::new("docker")
            .arg("exec")
            .arg("-i")
            .arg("spawn-db")
            .arg("psql")
            .arg("-U")
            .arg("spawn")
            .arg("spawn")
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
