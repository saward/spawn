use crate::config;
use crate::engine::EngineType;
use crate::escape::{EscapedIdentifier, EscapedLiteral};
use crate::secrets::{SecretsRenderMode, SecretsRepository};
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
use twox_hash::xxhash3_128;

/// Maps an EngineType to the appropriate SQL dialect for formatting.
///
/// Multiple engine types may share the same dialect. For example,
/// both a psql CLI engine and a native PostgreSQL driver would use
/// the Postgres dialect.
pub(crate) fn engine_to_dialect(engine: &EngineType) -> SqlDialect {
    match engine {
        EngineType::PostgresPSQL => SqlDialect::Postgres,
        // Future engines:
        // EngineType::PostgresNative => SqlDialect::Postgres,
        // EngineType::MySQL => SqlDialect::MySQL,
        // EngineType::SqlServer => SqlDialect::SqlServer,
    }
}

pub fn template_env(
    store: Store,
    engine: &EngineType,
    secrets: Arc<SecretsRepository>,
) -> Result<Environment<'static>> {
    let mut env = Environment::new();

    let store = Arc::new(store);

    let mj_store = MiniJinjaLoader {
        store: Arc::clone(&store),
    };
    env.set_loader(move |name: &str| mj_store.load(name));
    env.add_function("gen_uuid_v4", gen_uuid_v4);
    env.add_function("gen_uuid_v5", gen_uuid_v5);
    env.add_function("gen_uuid_v7", gen_uuid_v7);
    env.add_filter("escape_identifier", escape_identifier_filter);
    env.add_filter("escape_literal", escape_literal_filter);

    env.add_function(
        "secret",
        move |name: &str| -> Result<Value, minijinja::Error> { secret_function(name, &secrets) },
    );

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

fn gen_uuid_v7() -> Result<String, minijinja::Error> {
    Ok(Uuid::now_v7().to_string())
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

/// Filter to explicitly escape a value as a PostgreSQL literal (single-quoted string).
///
/// While Spawn auto-escapes template output as literals by default, this filter
/// is useful when you need to ensure a value is treated as a literal in contexts
/// where auto-escaping might not apply (e.g., after `safe` or inside macros).
///
/// Usage in templates: `{{ value|escape_literal }}`
fn escape_literal_filter(value: &Value) -> Result<Value, minijinja::Error> {
    let s = value.to_string();
    let escaped = EscapedLiteral::new(&s);
    // Return as a safe string so it won't be further escaped by the SQL formatter
    Ok(Value::from_safe_string(escaped.to_string()))
}

/// Resolves a named secret via the SecretsRepository, returning either its
/// real value or a masked placeholder depending on the current render mode.
///
/// Usage in templates: `{{ secret("application_password") }}`
fn secret_function(
    name: &str,
    secrets: &Arc<SecretsRepository>,
) -> Result<Value, minijinja::Error> {
    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async { secrets.resolve(name).await })
    });

    result.map(Value::from).map_err(|e| {
        minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, format!("{:#}", e))
    })
}

/// Reads raw bytes from a file in the components folder via the Store.
///
/// IDEA (not implemented): `read_file`/`read_json`/etc. re-fetch and
/// re-parse their input on every call, with no memoization across separate
/// render() invocations of the same template. minijinja's `Value` is
/// Arc-backed internally for its String/Bytes/Object variants (see
/// minijinja::value::ValueRepr), so cloning an already-built `Value` is
/// cheap — a filter-level cache keyed by input (e.g. on `Store`) could let
/// a second render of the same migration reuse an already-loaded large
/// file via a cheap Arc clone instead of re-fetching/re-parsing it. Only
/// worth building if a real need for multi-render or repeated large-file
/// loads comes up; nothing currently requires it.
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
    secrets: Arc<SecretsRepository>,
}

impl StreamingGeneration {
    /// Hex-encoded fingerprint of the migration's raw (unrendered) template
    /// source, for use as a migration_history audit-trail checksum.
    ///
    /// Deliberately hashes the raw source rather than the rendered output:
    /// rendered SQL may contain resolved secret values, which must never be
    /// persisted (or reconstructible from what's persisted) via the
    /// checksum. As a side effect, this also means secret rotation never
    /// changes a migration's recorded checksum.
    pub fn raw_checksum(&self) -> String {
        let hash = xxhash3_128::Hasher::oneshot(self.template_contents.as_bytes());
        format!("{:032x}", hash)
    }

    /// Render the template to the provided writer.
    /// This creates the minijinja environment and renders in one step.
    pub fn render_to_writer<W: std::io::Write + ?Sized>(self, writer: &mut W) -> Result<()> {
        let mut env = template_env(self.store, &self.engine, self.secrets)?;
        env.add_template("migration.sql", &self.template_contents)?;
        let tmpl = env.get_template("migration.sql")?;
        tmpl.render_to_write(
            context!(env => self.environment, variables => self.variables),
            writer,
        )?;
        Ok(())
    }

    /// Convert this streaming generation into a WriterFn that can be passed
    /// to migration_apply, along with a handle to the same secrets
    /// repository the render will use. That handle stays readable after the
    /// closure runs (e.g. once psql has exited), so a caller whose apply
    /// failed can find out which secret values were actually resolved and
    /// redact them from captured output before it's displayed or logged.
    pub fn into_writer_fn(self) -> (crate::engine::WriterFn, Arc<SecretsRepository>) {
        let secrets = Arc::clone(&self.secrets);
        let write_fn = Box::new(move |writer: &mut dyn std::io::Write| {
            self.render_to_writer(writer)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
        });
        (write_fn, secrets)
    }
}

/// Generate a streaming migration that can be rendered directly to a writer.
/// This avoids materializing the entire SQL in memory.
pub async fn generate_streaming(
    cfg: &config::Config,
    lock_file: Option<String>,
    name: &str,
    variables: Option<Variables>,
    secrets_mode: SecretsRenderMode,
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
    let target_config = cfg
        .target_config()
        .context("could not get target config for generate")?;
    let secrets = SecretsRepository::new(
        cfg.secrets.clone(),
        target_config.environment.clone(),
        secrets_mode,
        cfg.operator().clone(),
    );

    generate_streaming_with_store(
        name,
        variables,
        &target_config.environment,
        &target_config.engine,
        store,
        secrets,
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
    secrets: SecretsRepository,
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
        secrets: Arc::new(secrets),
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
        let op = Operator::new(mem_service).unwrap();
        op.write("components/test.txt", "file contents here")
            .await
            .unwrap();

        let pinner = Latest::new("").unwrap();
        let pather = FolderPather {
            spawn_folder: "".to_string(),
        };
        let secrets = SecretsRepository::empty(op.clone());
        let store = Store::new(Box::new(pinner), op, pather).unwrap();

        let mut env = template_env(store, &EngineType::PostgresPSQL, Arc::new(secrets)).unwrap();
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
        let op = Operator::new(mem_service).unwrap();
        op.write("components/binary.dat", vec![0xDE, 0xAD, 0xBE, 0xEF])
            .await
            .unwrap();

        let pinner = Latest::new("").unwrap();
        let pather = FolderPather {
            spawn_folder: "".to_string(),
        };
        let secrets = SecretsRepository::empty(op.clone());
        let store = Store::new(Box::new(pinner), op, pather).unwrap();

        let mut env = template_env(store, &EngineType::PostgresPSQL, Arc::new(secrets)).unwrap();
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
        let op = Operator::new(mem_service).unwrap();

        let pinner = Latest::new("").unwrap();
        let pather = FolderPather {
            spawn_folder: "".to_string(),
        };
        let secrets = SecretsRepository::empty(op.clone());
        let store = Store::new(Box::new(pinner), op, pather).unwrap();

        let mut env = template_env(store, &EngineType::PostgresPSQL, Arc::new(secrets)).unwrap();
        env.add_template(
            "test.sql",
            r#"{{ "nonexistent.txt"|read_file|to_string_lossy }}"#,
        )
        .unwrap();
        let tmpl = env.get_template("test.sql").unwrap();
        let result = tmpl.render(context!());
        assert!(result.is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_read_file_filter_uses_pinned_store() {
        use crate::config::FolderPather;
        use crate::store::pinner::snapshot;
        use crate::store::pinner::spawn::Spawn;
        use opendal::services::Memory;
        use opendal::Operator;

        let mem_service = Memory::default();
        let op = Operator::new(mem_service).unwrap();

        // Write a file into the components folder and snapshot it into the pinned store
        op.write("components/test.txt", "pinned content")
            .await
            .unwrap();
        let root_hash = snapshot(&op, Some("pinned/"), "components/").await.unwrap();

        // Delete the original file so it only exists in the pinned CAS store
        op.delete("components/test.txt").await.unwrap();

        // Create a Spawn pinner using the snapshot hash
        let pinner = Spawn::new_with_root_hash(
            "pinned/".to_string(),
            "components/".to_string(),
            &root_hash,
            &op,
        )
        .await
        .unwrap();

        let pather = FolderPather {
            spawn_folder: "".to_string(),
        };
        let secrets = SecretsRepository::empty(op.clone());
        let store = Store::new(Box::new(pinner), op, pather).unwrap();

        let mut env = template_env(store, &EngineType::PostgresPSQL, Arc::new(secrets)).unwrap();
        env.add_template(
            "test.sql",
            r#"{{ "test.txt"|read_file|to_string_lossy|safe }}"#,
        )
        .unwrap();
        let tmpl = env.get_template("test.sql").unwrap();
        let result = tmpl.render(context!()).unwrap();
        assert_eq!(result, "pinned content");
    }

    fn env_with_secrets(secrets: SecretsRepository) -> Environment<'static> {
        use crate::config::FolderPather;
        use opendal::services::Memory;
        use opendal::Operator;

        let op = Operator::new(Memory::default()).unwrap();
        let pinner = Latest::new("").unwrap();
        let pather = FolderPather {
            spawn_folder: "".to_string(),
        };
        let store = Store::new(Box::new(pinner), op, pather).unwrap();
        template_env(store, &EngineType::PostgresPSQL, Arc::new(secrets)).unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_secret_function_reveals_when_revealed_mode() {
        use std::collections::HashMap;

        let op = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
        let mut definitions = HashMap::new();
        definitions.insert(
            "application_password".to_string(),
            crate::secrets::SecretDefinition {
                default: crate::secrets::SecretSource::Literal {
                    value: "hunter2".to_string(),
                    insecure: true,
                },
                environments: HashMap::new(),
            },
        );
        let secrets = SecretsRepository::new(
            definitions,
            "prod".to_string(),
            SecretsRenderMode::Revealed,
            op,
        );

        let mut env = env_with_secrets(secrets);
        env.add_template("test.sql", r#"{{ secret("application_password") }}"#)
            .unwrap();
        let tmpl = env.get_template("test.sql").unwrap();
        let result = tmpl.render(context!()).unwrap();
        assert_eq!(result, "'hunter2'");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_secret_function_escapes_injection_attempt_when_revealed() {
        use std::collections::HashMap;

        let op = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
        let mut definitions = HashMap::new();
        definitions.insert(
            "application_password".to_string(),
            crate::secrets::SecretDefinition {
                default: crate::secrets::SecretSource::Literal {
                    value: "'; DROP TABLE users; --".to_string(),
                    insecure: true,
                },
                environments: HashMap::new(),
            },
        );
        let secrets = SecretsRepository::new(
            definitions,
            "prod".to_string(),
            SecretsRenderMode::Revealed,
            op,
        );

        let mut env = env_with_secrets(secrets);
        env.add_template(
            "test.sql",
            r#"CREATE ROLE app_user WITH LOGIN PASSWORD {{ secret("application_password") }};"#,
        )
        .unwrap();
        let tmpl = env.get_template("test.sql").unwrap();
        let result = tmpl.render(context!()).unwrap();
        assert_eq!(
            result,
            "CREATE ROLE app_user WITH LOGIN PASSWORD '''; DROP TABLE users; --';"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_secret_function_masks_when_masked_mode() {
        use std::collections::HashMap;

        let op = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
        let mut definitions = HashMap::new();
        definitions.insert(
            "application_password".to_string(),
            crate::secrets::SecretDefinition {
                default: crate::secrets::SecretSource::Literal {
                    value: "hunter2".to_string(),
                    insecure: true,
                },
                environments: HashMap::new(),
            },
        );
        let secrets = SecretsRepository::new(
            definitions,
            "prod".to_string(),
            SecretsRenderMode::Masked,
            op,
        );

        let mut env = env_with_secrets(secrets);
        env.add_template("test.sql", r#"{{ secret("application_password") }}"#)
            .unwrap();
        let tmpl = env.get_template("test.sql").unwrap();
        let result = tmpl.render(context!()).unwrap();
        assert_eq!(result, "'***MASKED:application_password***'");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_secret_function_errors_for_unknown_secret() {
        let op = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
        let secrets = SecretsRepository::empty(op);
        let mut env = env_with_secrets(secrets);
        env.add_template("test.sql", r#"{{ secret("nope") }}"#)
            .unwrap();
        let tmpl = env.get_template("test.sql").unwrap();
        let result = tmpl.render(context!());
        assert!(result.is_err());
    }

    fn streaming_generation(template_contents: &str, secrets: SecretsRepository) -> StreamingGeneration {
        use crate::config::FolderPather;
        use opendal::services::Memory;
        use opendal::Operator;

        let op = Operator::new(Memory::default()).unwrap();
        let pinner = Latest::new("").unwrap();
        let pather = FolderPather {
            spawn_folder: "".to_string(),
        };
        let store = Store::new(Box::new(pinner), op, pather).unwrap();

        StreamingGeneration {
            store,
            template_contents: template_contents.to_string(),
            environment: "prod".to_string(),
            variables: crate::variables::Variables::default(),
            engine: EngineType::PostgresPSQL,
            secrets: Arc::new(secrets),
        }
    }

    fn literal_secret_repo(value: &str) -> SecretsRepository {
        use std::collections::HashMap;

        let op = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
        let mut definitions = HashMap::new();
        definitions.insert(
            "application_password".to_string(),
            crate::secrets::SecretDefinition {
                default: crate::secrets::SecretSource::Literal {
                    value: value.to_string(),
                    insecure: true,
                },
                environments: HashMap::new(),
            },
        );
        SecretsRepository::new(
            definitions,
            "prod".to_string(),
            SecretsRenderMode::Revealed,
            op,
        )
    }

    // Regression coverage for the checksum's security-critical properties
    // (see StreamingGeneration::raw_checksum's doc comment): it must be a
    // fingerprint of the raw template source specifically — not the
    // rendered output, and not something secret-independent-but-otherwise-
    // arbitrary either — so a future change can't silently start hashing
    // rendered SQL (and disclose secret values via the checksum) again.

    #[test]
    fn raw_checksum_matches_a_hash_of_the_raw_template_source() {
        let source = r#"SELECT {{ secret("application_password") }};"#;
        let gen = streaming_generation(source, literal_secret_repo("hunter2"));

        let expected = format!(
            "{:032x}",
            twox_hash::xxhash3_128::Hasher::oneshot(source.as_bytes())
        );
        assert_eq!(gen.raw_checksum(), expected);
    }

    #[test]
    fn raw_checksum_is_unchanged_by_a_rotated_secret_value() {
        let source = r#"SELECT {{ secret("application_password") }};"#;
        let gen_a = streaming_generation(source, literal_secret_repo("hunter2"));
        let gen_b = streaming_generation(source, literal_secret_repo("a-totally-different-value"));

        assert_eq!(
            gen_a.raw_checksum(),
            gen_b.raw_checksum(),
            "rotating a secret's resolved value must not change the recorded checksum"
        );
    }

    #[test]
    fn raw_checksum_changes_when_the_template_source_changes() {
        let gen_a = streaming_generation("SELECT 1;", literal_secret_repo("hunter2"));
        let gen_b = streaming_generation("SELECT 2;", literal_secret_repo("hunter2"));

        assert_ne!(
            gen_a.raw_checksum(),
            gen_b.raw_checksum(),
            "different up.sql content must produce different checksums"
        );
    }
}
