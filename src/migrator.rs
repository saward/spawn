use crate::config;
use crate::pinfile::LockData;
use crate::store::{self, Store};
use crate::template;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;

use anyhow::{Context, Result};
use minijinja::{context, Environment};
use serde::Serialize;

static BASE_MIGRATION: &str = "BEGIN;

COMMIT;
";

/// Final SQL output generator
#[derive(Debug)]
pub struct Migrator {
    config: config::Config,
    /// Path for the script itself, set to the location under the migrations
    /// folder.
    script_path: OsString,
    /// Whether to use pinned components
    use_pinned: bool,
}

#[derive(Clone, Debug)]
pub enum Variables {
    Json(serde_json::Value),
    Toml(toml::Value),
    Yaml(serde_yaml::Value),
}

impl Serialize for Variables {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Variables::Json(v) => v.serialize(serializer),
            Variables::Toml(v) => v.serialize(serializer),
            Variables::Yaml(v) => v.serialize(serializer),
        }
    }
}

impl Default for Variables {
    fn default() -> Self {
        Self::Json(serde_json::Value::default())
    }
}

impl FromStr for Variables {
    type Err = String;

    fn from_str(path_str: &str) -> Result<Self, Self::Err> {
        let path = Path::new(path_str);
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read file {}: {}", path_str, e))?;

        match path.extension().and_then(|s| s.to_str()) {
            Some("json") => {
                let value: serde_json::Value =
                    serde_json::from_str(&content).map_err(|e| format!("Invalid JSON: {}", e))?;
                Ok(Variables::Json(value))
            }
            Some("toml") => {
                let value: toml::Value =
                    toml::from_str(&content).map_err(|e| format!("Invalid TOML: {}", e))?;
                Ok(Variables::Toml(value))
            }
            Some("yaml") | Some("yml") => {
                let value: serde_yaml::Value =
                    serde_yaml::from_str(&content).map_err(|e| format!("Invalid YAML: {}", e))?;
                Ok(Variables::Yaml(value))
            }
            _ => Err("Unsupported file format (expected .json, .toml, or .yaml)".into()),
        }
    }
}

impl Migrator {
    pub fn new(config: &config::Config, script_path: OsString, use_pinned: bool) -> Self {
        Migrator {
            config: config.clone(),
            script_path,
            use_pinned,
        }
    }

    /// Creates the migration folder with blank setup.
    pub fn create_migration(&self) -> Result<()> {
        // Todo: return error if migration already exists.
        let path = self.config.migration_folder(&self.script_path);
        if path.exists() {
            return Err(anyhow::anyhow!(
                "folder for migration {:?} already exists, aborting.",
                path,
            ));
        }
        fs::create_dir_all(&path)?;

        // Create our blank script file:
        fs::write(&path.join("script.sql"), BASE_MIGRATION)?;

        Ok(())
    }

    pub fn script_file_path(&self) -> Result<PathBuf> {
        let path = self.config.migration_script_file_path(&self.script_path);
        Ok(path
            .canonicalize()
            .context(format!("Invalid script path for '{:?}'", path))?)
    }

    /// Opens the specified script file and generates a migration script, compiled
    /// using minijinja.
    pub fn generate(&self, variables: Option<Variables>) -> Result<Generation> {
        // Create and set up the component loader
        let store = if self.use_pinned {
            println!("using pinned");
            let lock = self
                .config
                .load_lock_file(&self.script_path)
                .context("could not load pinned files lock file")?;
            let store = store::PinStore::new(self.config.pinned_folder(), lock.pin)?;
            let store: Arc<dyn Store + Send + Sync> = Arc::new(store);
            store
        } else {
            let store = store::LiveStore::new(self.config.components_folder())?;
            let store: Arc<dyn Store + Send + Sync> = Arc::new(store);
            store
        };

        // let store_clone: String = store.clone();

        let mut env = template::template_env(store)?;

        // Add our migration script to environment:
        let full_script_path = self.script_file_path()?;

        let contents = std::fs::read_to_string(&full_script_path).context(format!(
            "Failed to read migration script '{}'",
            full_script_path.display()
        ))?;
        env.add_template("migration.sql", &contents)?;

        // Render with provided variables
        let tmpl = env.get_template("migration.sql")?;
        let content = tmpl.render(
            context!(env => self.config.environment, variables => variables.unwrap_or_default()),
        )?;

        let result = Generation {
            content: content.to_string(),
        };

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use crate::migrator::Migrator;
    use std::{ffi::OsString, path::PathBuf};

    // fn test_config() -> Migrator {
    //     Migrator::new(
    //         PathBuf::from("./base_folder"),
    //         OsString::from("subfolder/migration_script"),
    //         false,
    //     )
    // }
    //
    // #[test]
    // fn script_file_path() {
    //     let config = test_config();
    //     assert_eq!(
    //         PathBuf::from("./base_folder/migrations/subfolder/migration_script"),
    //         config.script_file_path(),
    //     );
    // }
    //
    // #[test]
    // fn lock_file_path() {
    //     let config = test_config();
    //     assert_eq!(
    //         PathBuf::from("./base_folder/migrations/subfolder/migration_script.lock"),
    //         config.lock_file_path(),
    //     );
    // }
}

pub struct Generation {
    pub content: String,
}
