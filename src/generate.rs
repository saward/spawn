use crate::pinfile::LockData;
use crate::template::ComponentLoader;
use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use minijinja::{context, Environment};

/// Configuration for template generation
#[derive(Debug)]
pub struct GenerateConfig {
    /// Base path for all migration related files
    base_path: PathBuf,
    /// Path for the script itself, set to the location under the migrations
    /// folder.
    script_path: OsString,
    /// Variables to pass to the template
    variables: HashMap<String, String>,
    /// Whether to use pinned components
    use_pinned: bool,
}

impl GenerateConfig {
    pub fn new(
        base_path: PathBuf,
        script_path: OsString,
        variables: Option<HashMap<String, String>>,
        use_pinned: bool,
    ) -> Self {
        GenerateConfig {
            base_path,
            script_path,
            variables: variables.unwrap_or_default(),
            use_pinned,
        }
    }

    // temp_config is to be replaced eventually with a proper way of filling
    // this out.  For now, a single function that returns the config so that we
    // can test, and easily find all places to replace later.
    pub fn temp_config(migration: &OsString, use_pinned: bool) -> Self {
        GenerateConfig::new(
            PathBuf::from("./static/example"),
            migration.clone(),
            None,
            use_pinned,
        )
    }

    pub fn pinned_folder(&self) -> PathBuf {
        self.base_path.join("pinned")
    }

    pub fn components_folder(&self) -> PathBuf {
        self.base_path.join("components")
    }

    pub fn migrations_folder(&self) -> PathBuf {
        self.base_path.join("migrations")
    }

    pub fn script_file_path(&self) -> PathBuf {
        self.migrations_folder()
            .join(self.script_path.clone())
            .join("script.sql")
    }

    pub fn lock_file_path(&self) -> PathBuf {
        // Nightly has an add_extension that might be good to use one day if it
        // enters stable.
        let mut lock_file_name = self.script_path.clone();
        lock_file_name.push("components.lock");

        self.migrations_folder()
            .join(self.script_path.clone())
            .join("components.lock")
    }

    fn load_lock_file(&self) -> Result<LockData> {
        let lock_file = self.lock_file_path();
        let contents = fs::read_to_string(lock_file)?;
        let lock_data: LockData = toml::from_str(&contents)?;

        Ok(lock_data)
    }

    /// Opens the specified script file and generates a migration script, compiled
    /// using minijinja.
    pub fn generate(&self) -> Result<Generation> {
        let mut env = Environment::new();

        // Add our migration script to environment:
        let script_path = self.script_file_path().canonicalize().context(format!(
            "Invalid script path for '{:?}'",
            self.script_file_path()
        ))?;

        let contents = std::fs::read_to_string(&script_path).context(format!(
            "Failed to read migration script '{}'",
            script_path.display()
        ))?;
        env.add_template("migration.sql", &contents)?;

        // Create and set up the component loader
        let lock_data = if self.use_pinned {
            Some(
                self.load_lock_file()
                    .context("could not load pinned files lock file")?,
            )
        } else {
            None
        };

        let loader = Arc::new(ComponentLoader::new(
            self.components_folder(),
            self.pinned_folder(),
            lock_data,
        ));
        let loader_for_closure = loader.clone();
        env.set_loader(move |name| loader_for_closure.load(name));

        // Render with provided variables
        let tmpl = env.get_template("migration.sql")?;
        let content = tmpl.render(context!(variables => self.variables))?;

        // Print which files were loaded (for debugging/verification)
        if cfg!(debug_assertions) {
            eprintln!("Loaded files during template rendering:");
            for (name, _) in loader.get_loaded_files() {
                eprintln!("  - {}", name);
            }
        }

        let result = Generation {
            content: content.to_string(),
            files: loader.get_loaded_files(),
        };

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use crate::generate::GenerateConfig;
    use std::{ffi::OsString, path::PathBuf};

    fn test_config() -> GenerateConfig {
        GenerateConfig::new(
            PathBuf::from("./base_folder"),
            OsString::from("subfolder/migration_script"),
            None,
            false,
        )
    }

    #[test]
    fn script_file_path() {
        let config = test_config();
        assert_eq!(
            PathBuf::from("./base_folder/migrations/subfolder/migration_script"),
            config.script_file_path(),
        );
    }

    #[test]
    fn lock_file_path() {
        let config = test_config();
        assert_eq!(
            PathBuf::from("./base_folder/migrations/subfolder/migration_script.lock"),
            config.lock_file_path(),
        );
    }
}

pub struct Generation {
    pub content: String,
    pub files: HashMap<String, String>,
}
