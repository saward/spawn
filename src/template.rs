use crate::config;
use crate::store::pinner::latest::Latest;
use crate::store::pinner::spawn::Spawn;
use crate::store::pinner::Pinner;
use crate::store::Store;
use crate::template;
use crate::variables::Variables;
use minijinja::Environment;

use opendal::Operator;
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
            tokio::runtime::Handle::current()
                .block_on(async { self.store.load_component(name).await })
        });

        result.map_err(|e| {
            minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                format!("Failed to load from object store: {}", e),
            )
        })
    }
}

pub async fn generate(
    cfg: &config::Config,
    lock_file: Option<String>,
    name: &str,
    variables: Option<Variables>,
) -> Result<Generation> {
    let pinner: Box<dyn Pinner> = if let Some(lock_file) = lock_file {
        let lock = cfg
            .load_lock_file(&lock_file)
            .context("could not load pinned files lock file")?;
        let pinner = Spawn::new_with_root_hash(
            &cfg.pinned_folder(),
            &cfg.components_folder(),
            &lock.pin,
            &cfg.operator(),
        )
        .await?;
        Box::new(pinner)
    } else {
        let pinner = Latest::new()?;
        Box::new(pinner)
    };

    let store = Store::new(pinner, cfg.operator().clone())?;
    let db_config = cfg.db_config()?;

    generate_with_store(name, variables, &db_config.environment, store).await
}

pub async fn generate_with_store(
    name: &str,
    variables: Option<Variables>,
    environment: &str,
    store: Store,
) -> Result<Generation> {
    // Read contents from our object store first:
    let contents = store.load_migration(name).await?;

    // Create template environment
    let mut env = template::template_env(store)?;

    // Add our main script to environment:
    env.add_template("migration.sql", &contents)?;

    // Render with provided variables
    let tmpl = env.get_template("migration.sql")?;
    let content =
        tmpl.render(context!(env => environment, variables => variables.unwrap_or_default()))?;

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
