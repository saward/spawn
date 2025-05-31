use crate::pinfile::LockData;
use crate::store::{self, Store};
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context, Result};
use minijinja::{context, Environment};
use serde::Serialize;

static BASE_MIGRATION: &str = "BEGIN;

COMMIT;
";

static PINFILE_LOCK_NAME: &str = "lock.toml";

/// Final SQL output generator
#[derive(Debug)]
pub struct Migrator {
    /// Base path for all migration related files
    base_path: PathBuf,
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
    pub fn new(base_path: PathBuf, script_path: OsString, use_pinned: bool) -> Self {
        Migrator {
            base_path,
            script_path,
            use_pinned,
        }
    }

    // temp_config is to be replaced eventually with a proper way of filling
    // this out.  For now, a single function that returns the config so that we
    // can test, and easily find all places to replace later.
    pub fn temp_config(migration: &OsString, use_pinned: bool) -> Self {
        Migrator::new(
            PathBuf::from("./static/example"),
            migration.clone(),
            use_pinned,
        )
    }

    /// Creates the migration folder with blank setup.
    pub fn create_migration(&self) -> Result<()> {
        // Todo: return error if migration already exists.
        let path = self.migration_folder();
        if path.exists() {
            return Err(anyhow::anyhow!(
                "folder for migration {:?} already exists, aborting.",
                path,
            ));
        }
        fs::create_dir_all(self.migration_folder())?;

        // Create our blank script file:
        fs::write(path.join("script.sql"), BASE_MIGRATION)?;

        Ok(())
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

    pub fn migration_folder(&self) -> PathBuf {
        self.migrations_folder().join(&self.script_path)
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
        lock_file_name.push(PINFILE_LOCK_NAME);

        self.migrations_folder()
            .join(self.script_path.clone())
            .join(PINFILE_LOCK_NAME)
    }

    fn load_lock_file(&self) -> Result<LockData> {
        let lock_file = self.lock_file_path();
        let contents = fs::read_to_string(lock_file)?;
        let lock_data: LockData = toml::from_str(&contents)?;

        Ok(lock_data)
    }

    /// Opens the specified script file and generates a migration script, compiled
    /// using minijinja.
    pub fn generate(&self, variables: Option<Variables>) -> Result<Generation> {
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
        let store = if self.use_pinned {
            println!("using pinned");
            let lock = self
                .load_lock_file()
                .context("could not load pinned files lock file")?;
            let store = store::PinStore::new(self.pinned_folder(), lock.pin)?;
            let store: Arc<dyn Store + Send + Sync> = Arc::new(store);
            store
        } else {
            let store = store::LiveStore::new(self.components_folder())?;
            let store: Arc<dyn Store + Send + Sync> = Arc::new(store);
            store
        };

        let store_clone = store.clone();

        env.set_loader(move |name: &str| store_clone.load(name));

        // Render with provided variables
        let tmpl = env.get_template("migration.sql")?;
        let content = tmpl.render(context!(variables => variables.unwrap_or_default()))?;

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

    fn test_config() -> Migrator {
        Migrator::new(
            PathBuf::from("./base_folder"),
            OsString::from("subfolder/migration_script"),
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
}
