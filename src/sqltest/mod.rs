use crate::config;
use std::ffi::OsString;
use std::path::PathBuf;

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
}
