use crate::config;
use crate::engine::EngineType;
use crate::escape::EscapedIdentifier;
use crate::store::pinner::latest::Latest;
use crate::store::pinner::spawn::Spawn;
use crate::store::pinner::Pinner;
use crate::store::Store;
use crate::variables::Variables;
use minijinja::{Environment, Value};

use crate::sql_formatter::SqlDialect;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use uuid::Uuid;

use anyhow::{Context, Result};
use minijinja::context;
use std::sync::Arc;

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

    let store = Arc::new(store);

    let mj_store = MiniJinjaLoader {
        store: Arc::clone(&store),
    };
    env.set_loader(move |name: &str| mj_store.load(name));
    env.add_function("gen_uuid_v4", gen_uuid_v4);
    env.add_function("gen_uuid_v5", gen_uuid_v5);
    env.add_filter("escape_identifier", escape_identifier_filter);

    let read_file_store = Arc::clone(&store);
    env.add_filter(
        "read_file",
        move |path: &str| -> Result<Value, minijinja::Error> {
            read_file_filter(path, &read_file_store)
        },
    );
    env.add_filter("base64_encode", base64_encode_filter);
    env.add_filter("to_string_lossy", to_string_lossy_filter);
    env.add_filter("parse_json", parse_json_filter);
    env.add_filter("parse_toml", parse_toml_filter);
    env.add_filter("parse_yaml", parse_yaml_filter);

    let read_json_store = Arc::clone(&store);
    env.add_filter(
        "read_json",
        move |path: &str| -> Result<Value, minijinja::Error> {
            let bytes = read_file_bytes(path, &read_json_store)?;
            let s = string_from_bytes(&bytes)?;
            parse_json_filter(&s)
        },
    );
    let read_toml_store = Arc::clone(&store);
    env.add_filter(
        "read_toml",
        move |path: &str| -> Result<Value, minijinja::Error> {
            let bytes = read_file_bytes(path, &read_toml_store)?;
            let s = string_from_bytes(&bytes)?;
            parse_toml_filter(&s)
        },
    );
    let read_yaml_store = Arc::clone(&store);
    env.add_filter(
        "read_yaml",
        move |path: &str| -> Result<Value, minijinja::Error> {
            let bytes = read_file_bytes(path, &read_yaml_store)?;
            let s = string_from_bytes(&bytes)?;
            parse_yaml_filter(&s)
        },
    );

    // Get the appropriate dialect for this engine
    let dialect = engine_to_dialect(engine);

    // Enable SQL auto-escaping for .sql files using the dialect-specific callback
    env.set_auto_escape_callback(crate::sql_formatter::get_auto_escape_callback(dialect));

    // Set custom formatter that handles SQL escaping based on the dialect
    env.set_formatter(crate::sql_formatter::get_formatter(dialect));

    Ok(env)
}

struct MiniJinjaLoader {
    pub store: Arc<Store>,
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

/// Reads raw bytes from a file in the components folder via the Store.
fn read_file_bytes(path: &str, store: &Arc<Store>) -> Result<Vec<u8>, minijinja::Error> {
    let bytes = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async { store.read_file_bytes(path).await })
    });

    bytes.map_err(|e| {
        minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("Failed to read file '{}': {}", path, e),
        )
    })
}

/// Converts raw bytes to a UTF-8 string, returning an error on invalid UTF-8.
fn string_from_bytes(bytes: &[u8]) -> Result<String, minijinja::Error> {
    String::from_utf8(bytes.to_vec()).map_err(|e| {
        minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("File is not valid UTF-8: {}", e),
        )
    })
}

/// Filter to read a file from the components folder and return its contents as raw bytes.
///
/// Returns a bytes Value that can be further processed with `base64_encode` or `to_string_lossy`.
///
/// Usage in templates: `{{ "path/to/file"|read_file|to_string_lossy }}`
fn read_file_filter(path: &str, store: &Arc<Store>) -> Result<Value, minijinja::Error> {
    Ok(Value::from_bytes(read_file_bytes(path, store)?))
}

/// Filter to encode a value as a base64 string.
///
/// Accepts both bytes (e.g. from `read_file`) and strings.
///
/// Usage in templates: `{{ "path/to/file"|read_file|base64_encode }}`
fn base64_encode_filter(value: &Value) -> Result<Value, minijinja::Error> {
    use minijinja::value::ValueKind;
    match value.kind() {
        ValueKind::Bytes => {
            let bytes = value.as_bytes().unwrap();
            Ok(Value::from(STANDARD.encode(bytes)))
        }
        ValueKind::String => Ok(Value::from(STANDARD.encode(value.as_str().unwrap()))),
        _ => Err(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            "base64_encode filter expects bytes or string input",
        )),
    }
}

/// Filter to convert bytes to a string, replacing invalid UTF-8 sequences.
/// If the value is already a string, it is returned as-is.
///
/// Usage in templates: `{{ "path/to/file.txt"|read_file|to_string_lossy }}`
fn to_string_lossy_filter(value: &Value) -> Result<Value, minijinja::Error> {
    use minijinja::value::ValueKind;
    match value.kind() {
        ValueKind::Bytes => {
            let bytes = value.as_bytes().unwrap();
            Ok(Value::from(String::from_utf8_lossy(bytes).into_owned()))
        }
        ValueKind::String => Ok(value.clone()),
        _ => Err(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            "to_string_lossy filter expects bytes or string input",
        )),
    }
}

/// Filter to parse a JSON string into a template value.
///
/// Usage in templates: `{{ "data.json"|read_file|to_string_lossy|parse_json }}`
fn parse_json_filter(value: &str) -> Result<Value, minijinja::Error> {
    let vars = Variables::from_str("json", value).map_err(|e| {
        minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("parse_json: {}", e),
        )
    })?;
    Ok(Value::from_serialize(&vars))
}

/// Filter to parse a TOML string into a template value.
///
/// Usage in templates: `{{ "config.toml"|read_file|to_string_lossy|parse_toml }}`
fn parse_toml_filter(value: &str) -> Result<Value, minijinja::Error> {
    let vars = Variables::from_str("toml", value).map_err(|e| {
        minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("parse_toml: {}", e),
        )
    })?;
    Ok(Value::from_serialize(&vars))
}

/// Filter to parse a YAML string into a template value.
///
/// Usage in templates: `{{ "data.yaml"|read_file|to_string_lossy|parse_yaml }}`
fn parse_yaml_filter(value: &str) -> Result<Value, minijinja::Error> {
    let vars = Variables::from_str("yaml", value).map_err(|e| {
        minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("parse_yaml: {}", e),
        )
    })?;
    Ok(Value::from_serialize(&vars))
}

pub struct Generation {
    pub content: String,
}

/// Holds all the data needed to render a template to a writer.
/// This struct is Send and can be moved into a WriterFn closure.
pub struct StreamingGeneration {
    store: Store,
    template_contents: String,
    environment: String,
    variables: Variables,
    engine: EngineType,
}

impl StreamingGeneration {
    /// Render the template to the provided writer.
    /// This creates the minijinja environment and renders in one step.
    pub fn render_to_writer<W: std::io::Write + ?Sized>(self, writer: &mut W) -> Result<()> {
        let mut env = template_env(self.store, &self.engine)?;
        env.add_template("migration.sql", &self.template_contents)?;
        let tmpl = env.get_template("migration.sql")?;
        tmpl.render_to_write(
            context!(env => self.environment, variables => self.variables),
            writer,
        )?;
        Ok(())
    }

    /// Convert this streaming generation into a WriterFn that can be passed to migration_apply.
    pub fn into_writer_fn(self) -> crate::engine::WriterFn {
        Box::new(move |writer: &mut dyn std::io::Write| {
            self.render_to_writer(writer)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
        })
    }
}

/// Generate a streaming migration that can be rendered directly to a writer.
/// This avoids materializing the entire SQL in memory.
pub async fn generate_streaming(
    cfg: &config::Config,
    lock_file: Option<String>,
    name: &str,
    variables: Option<Variables>,
) -> Result<StreamingGeneration> {
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

    generate_streaming_with_store(
        name,
        variables,
        &db_config.environment,
        &db_config.engine,
        store,
    )
    .await
}

/// Generate a streaming migration with an existing store.
pub async fn generate_streaming_with_store(
    name: &str,
    variables: Option<Variables>,
    environment: &str,
    engine: &EngineType,
    store: Store,
) -> Result<StreamingGeneration> {
    // Read contents from our object store first:
    let contents = store
        .load_migration(name)
        .await
        .context("generate_streaming_with_store could not read migration")?;

    Ok(StreamingGeneration {
        store,
        template_contents: contents,
        environment: environment.to_string(),
        variables: variables.unwrap_or_default(),
        engine: engine.clone(),
    })
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

    #[test]
    fn test_base64_encode_filter() {
        let bytes = Value::from_bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        let result = base64_encode_filter(&bytes).unwrap();
        assert_eq!(result.to_string(), "3q2+7w==");
    }

    #[test]
    fn test_base64_encode_filter_text() {
        let bytes = Value::from_bytes(b"hello world".to_vec());
        let result = base64_encode_filter(&bytes).unwrap();
        assert_eq!(result.to_string(), "aGVsbG8gd29ybGQ=");
    }

    #[test]
    fn test_base64_encode_filter_string() {
        let value = Value::from("hello world");
        let result = base64_encode_filter(&value).unwrap();
        assert_eq!(result.to_string(), "aGVsbG8gd29ybGQ=");
    }

    #[test]
    fn test_base64_encode_filter_rejects_other_types() {
        let value = Value::from(42);
        let result = base64_encode_filter(&value);
        assert!(result.is_err());
    }

    #[test]
    fn test_to_string_lossy_filter_valid_utf8() {
        let bytes = Value::from_bytes(b"hello world".to_vec());
        let result = to_string_lossy_filter(&bytes).unwrap();
        assert_eq!(result.to_string(), "hello world");
    }

    #[test]
    fn test_to_string_lossy_filter_invalid_utf8() {
        let bytes = Value::from_bytes(vec![0x68, 0x65, 0x6C, 0xFF, 0x6F]);
        let result = to_string_lossy_filter(&bytes).unwrap();
        let s = result.to_string();
        assert!(s.contains("hel"));
        assert!(s.contains('\u{FFFD}'));
        assert!(s.contains('o'));
    }

    #[test]
    fn test_to_string_lossy_filter_passes_through_string() {
        let value = Value::from("already a string");
        let result = to_string_lossy_filter(&value).unwrap();
        assert_eq!(result.to_string(), "already a string");
    }

    #[test]
    fn test_to_string_lossy_filter_rejects_other_types() {
        let value = Value::from(42);
        let result = to_string_lossy_filter(&value);
        assert!(result.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_read_file_filter_with_store() {
        use crate::config::FolderPather;
        use crate::store::pinner::latest::Latest;
        use opendal::services::Memory;
        use opendal::Operator;

        // Set up an in-memory operator with a test file in the components folder
        let mem_service = Memory::default();
        let op = Operator::new(mem_service).unwrap().finish();
        op.write("components/test.txt", "file contents here")
            .await
            .unwrap();

        let pinner = Latest::new("").unwrap();
        let pather = FolderPather {
            spawn_folder: "".to_string(),
        };
        let store = Store::new(Box::new(pinner), op, pather).unwrap();

        let mut env = template_env(store, &EngineType::PostgresPSQL).unwrap();
        env.add_template(
            "test.sql",
            r#"{{ "test.txt"|read_file|to_string_lossy|safe }}"#,
        )
        .unwrap();
        let tmpl = env.get_template("test.sql").unwrap();
        let result = tmpl.render(context!()).unwrap();
        assert_eq!(result, "file contents here");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_read_file_with_base64_encode() {
        use crate::config::FolderPather;
        use crate::store::pinner::latest::Latest;
        use opendal::services::Memory;
        use opendal::Operator;

        let mem_service = Memory::default();
        let op = Operator::new(mem_service).unwrap().finish();
        op.write("components/binary.dat", vec![0xDE, 0xAD, 0xBE, 0xEF])
            .await
            .unwrap();

        let pinner = Latest::new("").unwrap();
        let pather = FolderPather {
            spawn_folder: "".to_string(),
        };
        let store = Store::new(Box::new(pinner), op, pather).unwrap();

        let mut env = template_env(store, &EngineType::PostgresPSQL).unwrap();
        env.add_template(
            "test.sql",
            r#"{{ "binary.dat"|read_file|base64_encode|safe }}"#,
        )
        .unwrap();
        let tmpl = env.get_template("test.sql").unwrap();
        let result = tmpl.render(context!()).unwrap();
        assert_eq!(result, "3q2+7w==");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_read_file_missing_file_returns_error() {
        use crate::config::FolderPather;
        use crate::store::pinner::latest::Latest;
        use opendal::services::Memory;
        use opendal::Operator;

        let mem_service = Memory::default();
        let op = Operator::new(mem_service).unwrap().finish();

        let pinner = Latest::new("").unwrap();
        let pather = FolderPather {
            spawn_folder: "".to_string(),
        };
        let store = Store::new(Box::new(pinner), op, pather).unwrap();

        let mut env = template_env(store, &EngineType::PostgresPSQL).unwrap();
        env.add_template(
            "test.sql",
            r#"{{ "nonexistent.txt"|read_file|to_string_lossy }}"#,
        )
        .unwrap();
        let tmpl = env.get_template("test.sql").unwrap();
        let result = tmpl.render(context!());
        assert!(result.is_err());
    }
}
