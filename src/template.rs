use crate::config;
use crate::store::pinner::latest::Latest;
use crate::store::pinner::spawn::Spawn;
use crate::store::pinner::Pinner;
use crate::store::Store;
use crate::template;
use crate::variables::Variables;
use minijinja::Environment;
use object_store::local::LocalFileSystem;
use std::path::PathBuf;
use uuid::Uuid;

use anyhow::{Context, Result};
use minijinja::context;

pub fn template_env(store: Store) -> Result<Environment<'static>> {
    let mut env = Environment::new();

    let mj_store = MiniJinjaLoader { store };
    env.set_loader(move |name: &str| mj_store.load(name));
    env.add_function("gen_uuid_v4", gen_uuid_v4);

    Ok(env)
}

struct MiniJinjaLoader {
    pub store: Store,
}

impl MiniJinjaLoader {
    pub fn load(&self, name: &str) -> std::result::Result<Option<String>, minijinja::Error> {
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { self.store.load(name).await })
        });

        result.map_err(|e| {
            minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                format!("Failed to load from object store: {}", e),
            )
        })
    }
}

// fn mj_loader_from_store_loader(
//     &self,
//     name: &str,
// ) -> std::result::Result<Option<String>, minijinja::Error> {
//     self.pinner.load(name, &self.fs)
// }

pub fn generate(
    cfg: &config::Config,
    lock_file: Option<PathBuf>,
    contents: &String,
    variables: Option<Variables>,
) -> Result<Generation> {
    // Create and set up the component loader
    let pinner: Box<dyn Pinner> = if let Some(lock_file) = lock_file {
        let lock = cfg
            .load_lock_file(&lock_file)
            .context("could not load pinned files lock file")?;
        let pinner = Spawn::new(
            cfg.pinned_folder(),
            cfg.components_folder(),
            Some(&lock.pin),
        )?;
        Box::new(pinner)
    } else {
        let pinner = Latest::new()?;
        Box::new(pinner)
    };

    let fs = Box::new(LocalFileSystem::new_with_prefix(&cfg.spawn_folder)?);

    let store = Store::new(pinner, fs)?;

    let mut env = template::template_env(store)?;

    // Add our main script to environment:
    env.add_template("migration.sql", contents)?;

    let db_config = cfg.db_config()?;

    // Render with provided variables
    let tmpl = env.get_template("migration.sql")?;
    let content = tmpl.render(
        context!(env => db_config.environment, variables => variables.unwrap_or_default()),
    )?;

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
