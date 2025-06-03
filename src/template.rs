use crate::store::Store;
use std::sync::Arc;
use uuid::Uuid;

use anyhow::Result;
use minijinja::Environment;

pub fn template_env(store: Arc<dyn Store + Send + Sync>) -> Result<Environment<'static>> {
    let mut env = Environment::new();

    env.set_loader(move |name: &str| store.clone().load(name));
    env.add_function("gen_uuid_v4", gen_uuid_v4);

    Ok(env)
}

fn gen_uuid_v4() -> Result<String, minijinja::Error> {
    Ok(Uuid::new_v4().to_string())
}
