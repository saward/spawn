use crate::config;
use crate::store::{self, Store};
use crate::template;
use crate::variables::Variables;
use minijinja::Environment;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

use anyhow::{Context, Result};
use minijinja::context;

pub fn template_env(store: Arc<dyn Store + Send + Sync>) -> Result<Environment<'static>> {
    let mut env = Environment::new();

    env.set_loader(move |name: &str| store.clone().load(name));
    env.add_function("gen_uuid_v4", gen_uuid_v4);

    Ok(env)
}

pub fn generate(
    cfg: &config::Config,
    lock_file: Option<PathBuf>,
    contents: &String,
    variables: Option<Variables>,
) -> Result<Generation> {
    // Create and set up the component loader
    let store = if let Some(lock_file) = lock_file {
        let lock = cfg
            .load_lock_file(&lock_file)
            .context("could not load pinned files lock file")?;
        let store = store::PinStore::new(cfg.pinned_folder(), lock.pin)?;
        let store: Arc<dyn Store + Send + Sync> = Arc::new(store);
        store
    } else {
        let store = store::LiveStore::new(cfg.components_folder())?;
        let store: Arc<dyn Store + Send + Sync> = Arc::new(store);
        store
    };

    let mut env = template::template_env(store)?;

    // Add our main script to environment:
    env.add_template("migration.sql", contents)?;

    // Render with provided variables
    let tmpl = env.get_template("migration.sql")?;
    let content =
        tmpl.render(context!(env => cfg.environment, variables => variables.unwrap_or_default()))?;

    let result = Generation {
        content: content.to_string(),
    };

    Ok(result)
}

fn gen_uuid_v4() -> Result<String, minijinja::Error> {
    Ok(Uuid::new_v4().to_string())
}

pub struct Generation {
    pub content: String,
}
