use crate::engine::{postgres_psql::PSQL, Engine, EngineType, TargetConfig};
use crate::pinfile::LockData;
use crate::secrets::SecretDefinition;
use crate::variables::Variables;
use anyhow::{anyhow, Context, Result};
use opendal::Operator;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Default configuration file name.
pub const DEFAULT_CONFIG_FILE: &str = "spawn.toml";

static PINFILE_LOCK_NAME: &str = "lock.toml";

// 1. The "Blueprint" struct. Use this for Deserialization.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConfigLoaderSaver {
    pub environment: Option<String>,
    /// Unique project identifier for telemetry (UUID string)
    pub project_id: Option<String>,
    pub spawn_folder: String,
    pub target: Option<String>,
    pub targets: Option<HashMap<String, TargetConfig>>,
    /// Named secrets available to templates via the `secret()` function.
    pub secrets: Option<HashMap<String, SecretDefinition>>,
    /// Allows you to override the default template for test new with a
    /// custom one.
    pub test_template: Option<String>,
    /// Allows you to override the default template for migration new with a
    /// custom one.
    pub up_template: Option<String>,
    /// Set to false to disable telemetry
    #[serde(default = "default_telemetry", skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<bool>,
}

fn default_telemetry() -> Option<bool> {
    None
}

impl ConfigLoaderSaver {
    // 2. A method to transform the Loader into the actual Config
    pub fn build(self, base_fs: Operator, spawn_fs: Option<Operator>) -> Config {
        Config {
            environment: self.environment,
            project_id: self.project_id,
            spawn_folder: self.spawn_folder,
            target: self.target,
            targets: self.targets.unwrap_or_default(),
            secrets: self.secrets.unwrap_or_default(),
            test_template: self.test_template,
            up_template: self.up_template,
            telemetry: self.telemetry.unwrap_or(true),
            base_fs,
            spawn_fs,
        }
    }

    pub async fn load(
        path: &str,
        op: &Operator,
        target: Option<String>,
    ) -> Result<ConfigLoaderSaver> {
        let bytes = op
            .read(path)
            .await
            .context(format!("No config found at path '{}'", &path))?
            .to_bytes();
        let main_config = String::from_utf8(bytes.to_vec())?;
        let source = config::File::from_str(&main_config, config::FileFormat::Toml);

        let mut settings = config::Config::builder().add_source(source);

        // Used to override the version in a repo.  For example, if you want to have your own local dev variables for testing reasons, that can be in .gitignore.
        match op.read(path).await {
            Ok(data) => {
                let bytes = String::from_utf8(data.to_bytes().to_vec())?;
                let override_config = config::File::from_str(&bytes, config::FileFormat::Toml);
                settings = settings.add_source(override_config);
            }
            Err(e) => match e.kind() {
                // If file not found, no override.  But any other failure is
                // an error to attend to.
                opendal::ErrorKind::NotFound => {}
                _ => return Err(e.into()),
            },
        };

        let settings = settings
            // Add in settings from the environment (with a prefix of APP)
            // Eg.. `APP_DEBUG=1 ./target/app` would set the `debug` key
            .add_source(config::Environment::with_prefix("SPAWN"))
            .set_override_option("target", target)?
            .build()?
            .try_deserialize()?;

        Ok(settings)
    }

    pub async fn save(&self, path: &str, op: &Operator) -> Result<()> {
        let toml_content = toml::to_string(self)?;
        op.write(path, toml_content).await?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct FolderPather {
    pub spawn_folder: String,
}

impl FolderPather {
    pub fn spawn_folder_path(&self) -> &str {
        self.spawn_folder.as_ref()
    }

    pub fn pinned_folder(&self) -> String {
        let mut s = self.spawn_folder_path().to_string();
        s.push_str("/pinned");
        s
    }

    /// Returns the path provided with the spawn folder base path set.
    pub fn any_path(&self, path: &str) -> String {
        let mut s = self.spawn_folder_path().to_string();
        s.push('/');
        s.push_str(path);
        s
    }

    pub fn components_folder(&self) -> String {
        let mut s = self.spawn_folder_path().to_string();
        s.push_str("/components");
        s
    }

    pub fn migrations_folder(&self) -> String {
        let mut s = self.spawn_folder_path().to_string();
        s.push_str("/migrations");
        s
    }

    pub fn tests_folder(&self) -> String {
        let mut s = self.spawn_folder_path().to_string();
        s.push_str("/tests");
        s
    }

    pub fn migration_folder(&self, script_path: &str) -> String {
        let mut s = self.migrations_folder();
        s.push('/');
        s.push_str(script_path);
        s
    }

    pub fn migration_script_file_path(&self, script_path: &str) -> String {
        let mut s = self.migration_folder(script_path);
        s.push_str("/up.sql");
        s
    }

    pub fn test_folder(&self, test_path: &str) -> String {
        let mut s = self.tests_folder();
        s.push('/');
        s.push_str(test_path);
        s
    }

    pub fn test_file_path(&self, test_path: &str) -> String {
        let mut s = self.test_folder(test_path);
        s.push_str("/test.sql");
        s
    }

    pub fn migration_lock_file_path(&self, script_path: &str) -> String {
        let mut s = self.migrations_folder();
        s.push('/');
        s.push_str(script_path);
        s.push('/');
        s.push_str(PINFILE_LOCK_NAME);
        s
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub environment: Option<String>, // Override the environment for the target config
    /// Unique project identifier for telemetry (UUID string)
    pub project_id: Option<String>,
    spawn_folder: String,
    pub target: Option<String>,
    pub targets: HashMap<String, TargetConfig>,
    /// Named secrets available to templates via the `secret()` function.
    pub secrets: HashMap<String, SecretDefinition>,
    /// Allows you to override the default template for test new with a
    /// custom one.
    pub test_template: Option<String>,
    /// Allows you to override the default template for migration new with a
    /// custom one.
    pub up_template: Option<String>,
    /// Whether telemetry is enabled in config
    pub telemetry: bool,

    // base_fs is the operator we used to load config, and may be the one we use
    // for all other interactions too.
    base_fs: Operator,
    // spawn_fs, when set, is an operator that differs from the one we used to
    // load the config.  Usually this will happen when our config file points to
    // another filesystem/location that should be used for spawn.
    spawn_fs: Option<Operator>,
}

impl Config {
    pub fn pather(&self) -> FolderPather {
        FolderPather {
            spawn_folder: self.spawn_folder.clone(),
        }
    }

    pub async fn new_engine(&self) -> Result<Box<dyn Engine>> {
        let target_config = self.target_config()?;

        match target_config.engine {
            EngineType::PostgresPSQL => Ok(PSQL::new(&target_config).await?),
        }
    }

    pub fn target_config(&self) -> Result<TargetConfig> {
        let target_name = self.target.as_ref().ok_or(anyhow!("no target selected"))?;
        let mut conf = self
            .targets
            .get(target_name)
            .ok_or(anyhow!("no target defined with name '{}'", target_name,))?
            .clone();

        if let Some(env) = &self.environment {
            conf.environment = env.clone();
        }

        Ok(conf)
    }

    pub async fn load(path: &str, op: &Operator, target: Option<String>) -> Result<Config> {
        let config_loader = ConfigLoaderSaver::load(path, op, target).await?;
        Ok(config_loader.build(op.clone(), None))
    }

    pub fn operator(&self) -> &Operator {
        if let Some(spawn_fs) = &self.spawn_fs {
            &spawn_fs
        } else {
            &self.base_fs
        }
    }

    pub async fn load_lock_file(&self, lock_file_path: &str) -> Result<LockData> {
        crate::pinfile::load_lock_file(self.operator(), lock_file_path).await
    }

    /// Load variables from a file path.
    /// The file type is determined by the file extension.
    pub async fn load_variables_from_path(&self, path: &str) -> Result<Variables> {
        let content = self
            .operator()
            .read(path)
            .await
            .context(format!("Failed to read variables file '{}'", path))?
            .to_bytes();
        let content_str =
            String::from_utf8(content.to_vec()).context("Variables file is not valid UTF-8")?;

        let extension = path.split('.').last().unwrap_or("");
        Variables::from_str(extension, &content_str)
            .context(format!("Failed to parse variables file '{}'", path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opendal::services::Memory;

    #[tokio::test]
    async fn load_without_top_level_environment_does_not_manufacture_one() {
        let op = Operator::new(Memory::default()).unwrap();
        op.write(
            "spawn.toml",
            r#"
spawn_folder = "spawn"
target = "dev_target"

[targets.dev_target]
engine = "postgres-psql"
environment = "dev"
"#,
        )
        .await
        .unwrap();

        let config = Config::load("spawn.toml", &op, None).await.unwrap();

        // No top-level `environment` was set in spawn.toml, so this must stay
        // None rather than defaulting to "prod" — target_config() only
        // overrides the target's own environment when this is explicitly Some.
        assert_eq!(config.environment, None);
        assert_eq!(config.target_config().unwrap().environment, "dev");
    }
}
