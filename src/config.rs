use crate::engine::{postgres_psql::PSQL, DatabaseConfig, Engine, EngineType};
use crate::pinfile::LockData;
use anyhow::{anyhow, Context, Result};
use opendal::Operator;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

static PINFILE_LOCK_NAME: &str = "lock.toml";

// 1. The "Blueprint" struct. Use this for Deserialization.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ConfigLoaderSaver {
    pub spawn_folder: String,
    pub database: String,
    pub environment: Option<String>,
    pub databases: HashMap<String, DatabaseConfig>,
}

impl ConfigLoaderSaver {
    // 2. A method to transform the Loader into the actual Config
    pub fn build(self, base_fs: Operator, spawn_fs: Option<Operator>) -> Config {
        Config {
            spawn_folder: self.spawn_folder,
            database: self.database,
            environment: self.environment,
            databases: self.databases,
            base_fs,
            spawn_fs,
        }
    }

    pub async fn load(
        path: &str,
        op: &Operator,
        database: Option<String>,
    ) -> Result<ConfigLoaderSaver> {
        let bytes = op.read(path).await?.to_bytes();
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
            .set_override_option("database", database)?
            .set_default("environment", "prod")
            .context("could not set default environment")?
            .build()?
            .try_deserialize()
            .context("could not deserialise config struct")?;

        Ok(settings)
    }

    pub async fn save(&self, path: &str, op: &Operator) -> Result<()> {
        let toml_content = toml::to_string(self)?;
        op.write(path, toml_content).await?;
        Ok(())
    }
}

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
    spawn_folder: String,
    pub database: String,
    pub environment: Option<String>, // Override the environment for the db config
    pub databases: HashMap<String, DatabaseConfig>,

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
        let db_config = self.db_config()?;

        match db_config.engine {
            EngineType::PostgresPSQL => Ok(PSQL::new(&db_config).await?),
        }
    }

    pub fn db_config(&self) -> Result<DatabaseConfig> {
        let mut conf = self
            .databases
            .get(&self.database)
            .ok_or(anyhow!(
                "no database defined with name '{}'",
                &self.database
            ))?
            .clone();

        if let Some(env) = &self.environment {
            conf.environment = env.clone();
        }

        Ok(conf)
    }

    pub async fn load(path: &str, op: &Operator, database: Option<String>) -> Result<Config> {
        let config_loader = ConfigLoaderSaver::load(path, op, database).await?;
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
        let contents = self.operator().read(lock_file_path).await?.to_bytes();
        let contents = String::from_utf8(contents.to_vec())?;
        let lock_data: LockData = toml::from_str(&contents)?;

        Ok(lock_data)
    }
}
