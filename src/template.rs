use crate::config;
use crate::store::pinner::latest::Latest;
use crate::store::pinner::spawn::Spawn;
use crate::store::pinner::Pinner;
use crate::store::Store;
use crate::template;
use crate::variables::Variables;
use minijinja::value::ValueKind;
use minijinja::{AutoEscape, Environment};

use postgres_protocol::escape::escape_literal;
use uuid::Uuid;

use anyhow::{Context, Result};
use minijinja::context;

pub fn template_env(store: Store) -> Result<Environment<'static>> {
    let mut env = Environment::new();

    let mj_store = MiniJinjaLoader { store };
    env.set_loader(move |name: &str| mj_store.load(name));
    env.add_function("gen_uuid_v4", gen_uuid_v4);
    env.add_function("gen_uuid_v5", gen_uuid_v5);

    // Enable SQL auto-escaping for .sql files
    env.set_auto_escape_callback(|name| {
        if name.ends_with(".sql") {
            AutoEscape::Custom("sql")
        } else {
            AutoEscape::None
        }
    });

    // Set custom formatter that handles SQL escaping based on value type
    env.set_formatter(sql_escape_formatter);

    Ok(env)
}

/// Custom formatter that escapes values for safe SQL interpolation.
///
/// This formatter is invoked when auto-escape is enabled (for .sql templates).
/// It handles different value types appropriately:
/// - Strings: Escaped using PostgreSQL literal escaping (wrapped in quotes)
/// - Numbers: Output as-is (safe for SQL)
/// - Booleans: Converted to SQL TRUE/FALSE
/// - None/Undefined: Converted to NULL
/// - Other types: Escaped as strings
fn sql_escape_formatter(
    out: &mut minijinja::Output<'_>,
    state: &minijinja::State<'_, '_>,
    value: &minijinja::Value,
) -> Result<(), minijinja::Error> {
    // Check if we're in SQL auto-escape mode
    if state.auto_escape() == AutoEscape::Custom("sql") {
        // If the value is marked as safe (via |safe filter), skip escaping
        if value.is_safe() {
            return write!(out, "{}", value).map_err(|e| {
                minijinja::Error::new(minijinja::ErrorKind::WriteFailure, e.to_string())
            });
        }

        match value.kind() {
            ValueKind::Undefined => {
                // Undefined values render as empty string (consistent with minijinja default)
                Ok(())
            }
            ValueKind::None => write!(out, "NULL").map_err(|e| {
                minijinja::Error::new(minijinja::ErrorKind::WriteFailure, e.to_string())
            }),
            ValueKind::Bool => {
                let b: bool = value.clone().try_into().unwrap_or(false);
                write!(out, "{}", if b { "TRUE" } else { "FALSE" }).map_err(|e| {
                    minijinja::Error::new(minijinja::ErrorKind::WriteFailure, e.to_string())
                })
            }
            ValueKind::Number => {
                // Numbers are safe to output directly
                write!(out, "{}", value).map_err(|e| {
                    minijinja::Error::new(minijinja::ErrorKind::WriteFailure, e.to_string())
                })
            }
            ValueKind::String => {
                // Strings need SQL literal escaping
                let s: String = value.to_string();
                let escaped = escape_literal(&s);
                write!(out, "{}", escaped).map_err(|e| {
                    minijinja::Error::new(minijinja::ErrorKind::WriteFailure, e.to_string())
                })
            }
            _ => {
                // For complex types (Seq, Map, etc.), convert to string and escape
                let s = value.to_string();
                let escaped = escape_literal(&s);
                write!(out, "{}", escaped).map_err(|e| {
                    minijinja::Error::new(minijinja::ErrorKind::WriteFailure, e.to_string())
                })
            }
        }
    } else {
        // For non-SQL templates, use default formatting (no escaping)
        write!(out, "{}", value)
            .map_err(|e| minijinja::Error::new(minijinja::ErrorKind::WriteFailure, e.to_string()))
    }
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
            .await
            .context("could not load pinned files lock file")?;
        let pinner = Spawn::new_with_root_hash(
            cfg.pather().pinned_folder(),
            cfg.pather().components_folder(),
            &lock.pin,
            &cfg.operator(),
        )
        .await
        .context("could not get new root with hash")?;
        Box::new(pinner)
    } else {
        let pinner = Latest::new(cfg.pather().spawn_folder_path())?;
        Box::new(pinner)
    };

    let store = Store::new(pinner, cfg.operator().clone(), cfg.pather())
        .context("could not create new store for generate")?;
    let db_config = cfg
        .db_config()
        .context("could not get db config for generate")?;

    generate_with_store(name, variables, &db_config.environment, store).await
}

pub async fn generate_with_store(
    name: &str,
    variables: Option<Variables>,
    environment: &str,
    store: Store,
) -> Result<Generation> {
    // Read contents from our object store first:
    let contents = store
        .load_migration(name)
        .await
        .context("generate_with_store could not read migration")?;

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

fn gen_uuid_v5(seed: &str) -> Result<String, minijinja::Error> {
    Ok(Uuid::new_v5(&Uuid::NAMESPACE_DNS, seed.as_bytes()).to_string())
}

pub struct Generation {
    pub content: String,
}
