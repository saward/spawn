use crate::config;
use crate::engine::EngineType;
use crate::escape::EscapedIdentifier;
use crate::store::pinner::latest::Latest;
use crate::store::pinner::spawn::Spawn;
use crate::store::pinner::Pinner;
use crate::store::Store;
use crate::template;
use crate::variables::Variables;
use minijinja::{Environment, Value};

use crate::sql_formatter::SqlDialect;
use uuid::Uuid;

use anyhow::{Context, Result};
use minijinja::context;

/// Maps an EngineType to the appropriate SQL dialect for formatting.
///
/// Multiple engine types may share the same dialect. For example,
/// both a psql CLI engine and a native PostgreSQL driver would use
/// the Postgres dialect.
fn engine_to_dialect(engine: &EngineType) -> SqlDialect {
    match engine {
        EngineType::PostgresPSQL => SqlDialect::Postgres,
        // Future engines:
        // EngineType::PostgresNative => SqlDialect::Postgres,
        // EngineType::MySQL => SqlDialect::MySQL,
        // EngineType::SqlServer => SqlDialect::SqlServer,
    }
}

pub fn template_env(store: Store, engine: &EngineType) -> Result<Environment<'static>> {
    let mut env = Environment::new();

    let mj_store = MiniJinjaLoader { store };
    env.set_loader(move |name: &str| mj_store.load(name));
    env.add_function("gen_uuid_v4", gen_uuid_v4);
    env.add_function("gen_uuid_v5", gen_uuid_v5);
    env.add_filter("escape_identifier", escape_identifier_filter);

    // Get the appropriate dialect for this engine
    let dialect = engine_to_dialect(engine);

    // Enable SQL auto-escaping for .sql files using the dialect-specific callback
    env.set_auto_escape_callback(crate::sql_formatter::get_auto_escape_callback(dialect));

    // Set custom formatter that handles SQL escaping based on the dialect
    env.set_formatter(crate::sql_formatter::get_formatter(dialect));

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

    generate_with_store(
        name,
        variables,
        &db_config.environment,
        &db_config.engine,
        store,
    )
    .await
}

pub async fn generate_with_store(
    name: &str,
    variables: Option<Variables>,
    environment: &str,
    engine: &EngineType,
    store: Store,
) -> Result<Generation> {
    // Read contents from our object store first:
    let contents = store
        .load_migration(name)
        .await
        .context("generate_with_store could not read migration")?;

    // Create template environment with engine-specific formatting
    let mut env = template::template_env(store, engine)?;

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

/// Filter to escape a value as a PostgreSQL identifier (e.g., database name, table name).
///
/// This wraps the value in double quotes and escapes any embedded double quotes,
/// making it safe to use in SQL statements where an identifier is expected.
///
/// Usage in templates: `{{ dbname|escape_identifier }}`
fn escape_identifier_filter(value: &Value) -> Result<Value, minijinja::Error> {
    let s = value.to_string();
    let escaped = EscapedIdentifier::new(&s);
    // Return as a safe string so it won't be further escaped by the SQL formatter
    Ok(Value::from_safe_string(escaped.to_string()))
}

pub struct Generation {
    pub content: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql_formatter::{get_auto_escape_callback, get_formatter};
    use minijinja::{context, Environment, Value};

    /// Helper to test SQL formatting of a value by rendering it in a .sql template
    fn render_sql_value(value: Value) -> String {
        let mut env = Environment::new();
        env.set_auto_escape_callback(get_auto_escape_callback(SqlDialect::Postgres));
        env.set_formatter(get_formatter(SqlDialect::Postgres));
        env.add_template("test.sql", "{{ value }}").unwrap();
        let tmpl = env.get_template("test.sql").unwrap();
        tmpl.render(context!(value => value)).unwrap()
    }

    #[test]
    fn test_engine_to_dialect_postgres_psql() {
        let dialect = engine_to_dialect(&EngineType::PostgresPSQL);
        assert_eq!(dialect, SqlDialect::Postgres);
    }

    // Basic escaping tests - verify the integration with spawn-sql-format works
    // More comprehensive tests are in the spawn-sql-format crate itself

    #[test]
    fn test_sql_escape_string() {
        let result = render_sql_value(Value::from("hello"));
        assert_eq!(result, "'hello'");
    }

    #[test]
    fn test_sql_escape_string_injection_attempt() {
        let result = render_sql_value(Value::from("'; DROP TABLE users; --"));
        assert_eq!(result, "'''; DROP TABLE users; --'");
    }

    #[test]
    fn test_sql_escape_integer() {
        let result = render_sql_value(Value::from(42));
        assert_eq!(result, "42");
    }

    #[test]
    fn test_sql_escape_bool() {
        let result = render_sql_value(Value::from(true));
        assert_eq!(result, "TRUE");
    }

    #[test]
    fn test_sql_escape_none() {
        let result = render_sql_value(Value::from(()));
        assert_eq!(result, "NULL");
    }

    #[test]
    fn test_sql_escape_seq() {
        let result = render_sql_value(Value::from(vec![1, 2, 3]));
        assert_eq!(result, "ARRAY[1, 2, 3]");
    }

    #[test]
    fn test_sql_escape_bytes() {
        let bytes = Value::from_bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        let result = render_sql_value(bytes);
        assert_eq!(result, "'\\xdeadbeef'::bytea");
    }

    #[test]
    fn test_sql_escape_for_non_sql_templates() {
        let mut env = Environment::new();
        env.set_auto_escape_callback(get_auto_escape_callback(SqlDialect::Postgres));
        env.set_formatter(get_formatter(SqlDialect::Postgres));
        // Use .txt extension - should still trigger SQL escaping
        env.add_template("test.txt", "{{ value }}").unwrap();
        let tmpl = env.get_template("test.txt").unwrap();
        let result = tmpl.render(context!(value => "hello")).unwrap();
        // SQL escaping applies to all files
        assert_eq!(result, "'hello'");
    }

    #[test]
    fn test_sql_safe_filter_bypasses_escaping() {
        let mut env = Environment::new();
        env.set_auto_escape_callback(get_auto_escape_callback(SqlDialect::Postgres));
        env.set_formatter(get_formatter(SqlDialect::Postgres));
        // Using |safe filter should bypass escaping
        env.add_template("test.sql", "{{ value|safe }}").unwrap();
        let tmpl = env.get_template("test.sql").unwrap();
        let result = tmpl.render(context!(value => "raw SQL here")).unwrap();
        // Should be output as-is without quotes
        assert_eq!(result, "raw SQL here");
    }

    #[test]
    fn test_sql_escape_only_on_output_not_in_loops() {
        let mut env = Environment::new();
        env.set_auto_escape_callback(get_auto_escape_callback(SqlDialect::Postgres));
        env.set_formatter(get_formatter(SqlDialect::Postgres));

        let template =
            r#"{% for item in items %}{{ item }}{% if not loop.last %}, {% endif %}{% endfor %}"#;
        env.add_template("test.sql", template).unwrap();
        let tmpl = env.get_template("test.sql").unwrap();

        let items = vec!["alice", "bob", "charlie"];
        let result = tmpl.render(context!(items => items)).unwrap();
        assert_eq!(result, "'alice', 'bob', 'charlie'");
    }
}
