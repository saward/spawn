use crate::config;
use crate::engine::EngineType;
use crate::escape::{EscapedIdentifier, EscapedLiteral};
use crate::hash::content_hash;
use crate::secrets::{SecretsRenderMode, SecretsRepository};
use crate::source_config::SourceConfig;
use crate::store::pinner::latest::Latest;
use crate::store::pinner::spawn::Spawn;
use crate::store::pinner::Pinner;
use crate::store::Store;
use crate::variables::Variables;
use minijinja::{Environment, UndefinedBehavior, Value};

use crate::sql_formatter::SqlDialect;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use uuid::Uuid;

use anyhow::{anyhow, Context, Result};
use minijinja::context;
use serde::Serialize;
use std::sync::Arc;

/// What kind of thing is being rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderKind {
    Migration,
    Test,
    Script,
}

/// Values describing the current render, exposed to templates as globals
/// under the `builtin` namespace (e.g. `{{ builtin.script_name }}`).
#[derive(Debug, Clone, Serialize)]
pub struct BuiltinContext {
    /// Name of the migration, test or script being rendered, not the object-store file path.
    pub script_name: String,
    /// What kind of render this is.
    pub script_type: RenderKind,
    /// The target engine (e.g. `postgres-psql`).
    pub engine: EngineType,
}

impl BuiltinContext {
    pub fn new(
        script_name: impl Into<String>,
        script_type: RenderKind,
        engine: EngineType,
    ) -> Self {
        BuiltinContext {
            script_name: script_name.into(),
            script_type,
            engine,
        }
    }
}

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
    builtin: BuiltinContext,
) -> Result<Environment<'static>> {
    let mut env = Environment::new();

    // An unset variable is a mistake, not an empty string: rendering nothing
    // can leave valid SQL that acts on the wrong thing. SemiStrict rather than
    // Strict so `{% if variables.x %}` and `| default(..)` still cover
    // genuinely optional variables.
    env.set_undefined_behavior(UndefinedBehavior::SemiStrict);

    // Debug mode copies the values in scope — secrets included — into every
    // error it builds. Set explicitly because minijinja otherwise defaults it
    // to `cfg!(debug_assertions)`, so dev and release would differ.
    env.set_debug(false);

    env.add_global("builtin", Value::from_serialize(&builtin));

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

/// Longest expression reported in an error message, in characters.
const EXPRESSION_LIMIT: usize = 120;

/// The source text of the expression that failed, e.g. `variables.tenant`.
///
/// minijinja reports no variable name, only the byte range of the expression,
/// so the text is read back out of the template.
///
/// The range covers whichever operation raised, which is not always the
/// variable you forgot: `{{ v.x | upper }}` gives `upper`. Never present this
/// as a variable name.
fn undefined_expression(env: &Environment<'_>, err: &minijinja::Error) -> Option<String> {
    // An `{% include %}` wraps the real error in one pointing at the include
    // tag, so the innermost error is the useful one.
    let mut frame = err;
    while let Some(source) = std::error::Error::source(frame)
        .and_then(|source| source.downcast_ref::<minijinja::Error>())
    {
        frame = source;
    }

    if frame.kind() != minijinja::ErrorKind::UndefinedError {
        return None;
    }

    // Via the environment, not `Error::template_source`, which needs the debug
    // mode that `template_env` deliberately leaves off. Already compiled, so
    // this is a lookup rather than a re-render.
    let template = env.get_template(frame.name()?).ok()?;

    // `get` rather than `[..]`, which panics if the range does not line up.
    let expression = template.source().get(frame.range()?)?.trim();

    if expression.is_empty() {
        return None;
    }

    // Bounded because this ends up in logs: an expression can span lines, and
    // a newline would let a template split a log line.
    let mut flattened = expression.split_whitespace().collect::<Vec<_>>().join(" ");
    if flattened.chars().count() > EXPRESSION_LIMIT {
        flattened = flattened
            .chars()
            .take(EXPRESSION_LIMIT)
            .chain("...".chars())
            .collect();
    }

    Some(flattened)
}

/// Adds the failing expression to the message of an undefined-value error.
///
/// A `context` layer rather than a replacement, so the original error stays in
/// the chain — callers search it for an `io::Error` to spot a closed pipe.
fn name_undefined_expression(env: &Environment<'_>, err: minijinja::Error) -> anyhow::Error {
    match undefined_expression(env, &err) {
        Some(expression) => {
            anyhow::Error::new(err).context(format!("undefined value in expression `{expression}`"))
        }
        None => anyhow::Error::new(err),
    }
}

pub struct Generation {
    pub content: String,
}

/// Holds all the data needed to render a template to a writer.
/// This struct is Send and can be moved into a ScriptSource closure.
pub struct StreamingGeneration {
    store: Store,
    template_contents: String,
    /// The template's real path, used as its minijinja template name so render
    /// errors name the file on disk.
    template_label: String,
    /// Name of the migration, test or script, exposed as `builtin.script_name`.
    script_name: String,
    /// What kind of render this is, exposed as `builtin.script_type`.
    script_type: RenderKind,
    environment: String,
    variables: Variables,
    engine: EngineType,
    secrets: Arc<SecretsRepository>,
    /// The pin used to build this render's component store (`None` with
    /// `--no-pin`), captured when the source config was read — prefer it to
    /// re-reading that config, which could race a
    /// concurrent re-pin and record a hash that doesn't match what was
    /// actually rendered.
    pin_hash: Option<String>,
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
        content_hash(self.template_contents.as_bytes())
    }

    /// The pin actually used to build this render's component store — see
    /// the `pin_hash` field doc comment for why this should be preferred
    /// over separately re-reading the source config.
    pub fn pin_hash(&self) -> Option<&str> {
        self.pin_hash.as_deref()
    }

    /// Render the template to the provided writer.
    /// This creates the minijinja environment and renders in one step.
    pub fn render_to_writer<W: std::io::Write + ?Sized>(self, writer: &mut W) -> Result<()> {
        let builtin = BuiltinContext::new(
            self.script_name.clone(),
            self.script_type,
            self.engine.clone(),
        );
        let mut env = template_env(self.store, &self.engine, self.secrets, builtin)?;
        env.add_template(&self.template_label, &self.template_contents)?;
        let tmpl = env.get_template(&self.template_label)?;
        tmpl.render_to_write(
            context!(env => self.environment, variables => self.variables),
            writer,
        )
        .map_err(|err| name_undefined_expression(&env, err))?;
        Ok(())
    }

    /// Convert this streaming generation into a ScriptSource that can be passed
    /// to migration_apply, along with a handle to the same secrets
    /// repository the render will use. That handle stays readable after the
    /// closure runs (e.g. once psql has exited), so a caller whose apply
    /// failed can find out which secret values were actually resolved and
    /// redact them from captured output before it's displayed or logged.
    pub fn into_script_source(self) -> (crate::engine::ScriptSource, Arc<SecretsRepository>) {
        let secrets = Arc::clone(&self.secrets);
        let script = Box::new(move |writer: &mut dyn std::io::Write| {
            self.render_to_writer(writer).map_err(|e| {
                // minijinja buries the writer's io::Error under WriteFailure;
                // carry its kind back out so callers can spot a broken pipe.
                let kind = e
                    .chain()
                    .find_map(|c| c.downcast_ref::<std::io::Error>())
                    .map(|io| io.kind())
                    .unwrap_or(std::io::ErrorKind::Other);
                std::io::Error::new(kind, e)
            })
        });
        (script, secrets)
    }
}

/// Generate a streaming migration that can be rendered directly to a writer.
/// This avoids materializing the entire SQL in memory.
pub async fn generate_streaming(
    cfg: &config::Config,
    // A migration's lock.toml or a script's config.toml; None renders live.
    source_config_path: Option<String>,
    script_name: &str,
    script_type: RenderKind,
    path: &str,
    variables: Option<Variables>,
    secrets_mode: SecretsRenderMode,
) -> Result<StreamingGeneration> {
    let (pinner, pin_hash): (Box<dyn Pinner>, Option<String>) =
        if let Some(source_config_path) = source_config_path {
            let source_config: SourceConfig = cfg
                .load_source_config(&source_config_path)
                .await
                .context("could not load source config")?;
            // Callers only pass a path once they have established there is a
            // pin to render against, so a missing one here is a bug rather
            // than a user's unpinned migration or script.
            let pin = source_config.pin.ok_or_else(|| {
                anyhow!(
                    "source config '{}' records no pin to render against",
                    source_config_path
                )
            })?;
            let pinner = Spawn::new_with_root_hash(
                cfg.pather().pinned_folder(),
                cfg.pather().components_folder(),
                &pin,
                &cfg.operator(),
            )
            .await
            .context("could not get new root with hash")?;
            (Box::new(pinner), Some(pin))
        } else {
            let pinner = Latest::new(cfg.pather().spawn_folder_path())?;
            (Box::new(pinner), None)
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
        script_name,
        script_type,
        path,
        variables,
        &target_config.environment,
        &target_config.engine,
        store,
        secrets,
        pin_hash,
    )
    .await
}

/// Generate a streaming migration with an existing store.
pub async fn generate_streaming_with_store(
    script_name: &str,
    script_type: RenderKind,
    path: &str,
    variables: Option<Variables>,
    environment: &str,
    engine: &EngineType,
    store: Store,
    secrets: SecretsRepository,
    pin_hash: Option<String>,
) -> Result<StreamingGeneration> {
    // Read contents from our object store first:
    let contents = store
        .load_entry(path)
        .await
        .with_context(|| format!("could not read script at '{}'", path))?;

    Ok(StreamingGeneration {
        store,
        template_contents: contents,
        template_label: path.to_string(),
        script_name: script_name.to_string(),
        script_type,
        environment: environment.to_string(),
        variables: variables.unwrap_or_default(),
        engine: engine.clone(),
        secrets: Arc::new(secrets),
        pin_hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql_formatter::{get_auto_escape_callback, get_formatter};
    use minijinja::{context, Environment, Value};
    use serde_json::json;

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

        let mut env = template_env(
            store,
            &EngineType::PostgresPSQL,
            Arc::new(secrets),
            BuiltinContext::new("test", RenderKind::Migration, EngineType::PostgresPSQL),
        )
        .unwrap();
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

        let mut env = template_env(
            store,
            &EngineType::PostgresPSQL,
            Arc::new(secrets),
            BuiltinContext::new("test", RenderKind::Migration, EngineType::PostgresPSQL),
        )
        .unwrap();
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

        let mut env = template_env(
            store,
            &EngineType::PostgresPSQL,
            Arc::new(secrets),
            BuiltinContext::new("test", RenderKind::Migration, EngineType::PostgresPSQL),
        )
        .unwrap();
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

        let mut env = template_env(
            store,
            &EngineType::PostgresPSQL,
            Arc::new(secrets),
            BuiltinContext::new("test", RenderKind::Migration, EngineType::PostgresPSQL),
        )
        .unwrap();
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
        template_env(
            store,
            &EngineType::PostgresPSQL,
            Arc::new(secrets),
            BuiltinContext::new("test", RenderKind::Migration, EngineType::PostgresPSQL),
        )
        .unwrap()
    }

    /// Environment with no secrets, for tests about template-engine behaviour.
    fn plain_env() -> Environment<'static> {
        use opendal::services::Memory;
        use opendal::Operator;
        let op = Operator::new(Memory::default()).unwrap();
        env_with_secrets(SecretsRepository::empty(op))
    }

    fn render(source: &str, vars: serde_json::Value) -> Result<String, minijinja::Error> {
        let mut env = plain_env();
        env.add_template("test.sql", source).unwrap();
        let tmpl = env.get_template("test.sql").unwrap();
        tmpl.render(context!(variables => Value::from_serialize(&vars)))
    }

    // The next four tests pin down the SemiStrict undefined behaviour set in
    // `template_env`; Strict would also reject the truthiness test below.

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_printing_an_unset_variable_is_an_error() {
        let err = render("-- {{ variables.missing }}", serde_json::json!({}))
            .expect_err("printing an unset variable should fail, not render as empty");
        assert_eq!(err.kind(), minijinja::ErrorKind::UndefinedError);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_a_variable_set_to_empty_is_not_undefined() {
        // Quoted because the auto-escaping formatter makes a string a SQL literal.
        let out = render("-- {{ variables.blank }}", serde_json::json!({"blank": ""})).unwrap();
        assert_eq!(out, "-- ''");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_default_filter_still_rescues_an_unset_variable() {
        let out = render(
            r#"-- {{ variables.missing | default("fallback") }}"#,
            serde_json::json!({}),
        )
        .unwrap();
        assert_eq!(out, "-- 'fallback'");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_an_unset_variable_may_still_be_tested_for_truthiness() {
        let out = render(
            "{% if variables.flag %}on{% else %}off{% endif %}",
            serde_json::json!({}),
        )
        .unwrap();
        assert_eq!(out, "off");
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

    fn streaming_generation(
        template_contents: &str,
        secrets: SecretsRepository,
    ) -> StreamingGeneration {
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
            template_label: "migrations/test/up.sql".to_string(),
            script_name: "test".to_string(),
            script_type: RenderKind::Migration,
            environment: "prod".to_string(),
            variables: crate::variables::Variables::default(),
            engine: EngineType::PostgresPSQL,
            secrets: Arc::new(secrets),
            pin_hash: None,
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

    #[test]
    fn builtin_context_is_available_in_templates() {
        let gen = streaming_generation(
            "{{ builtin.script_name }} {{ builtin.script_type }} {{ builtin.engine }}",
            literal_secret_repo("hunter2"),
        );

        let mut buffer = Vec::new();
        gen.render_to_writer(&mut buffer).unwrap();
        // SQL auto-escaping wraps string output in literal quotes.
        assert_eq!(
            String::from_utf8(buffer).unwrap(),
            "'test' 'migration' 'postgres-psql'"
        );
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

        let expected = content_hash(source.as_bytes());
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

    /// minijinja hides an `{% include %}` failure's real cause behind a
    /// `BadInclude` wrapper, reachable only via `.source()`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn include_failure_keeps_the_real_cause_reachable() {
        use crate::config::FolderPather;
        use opendal::services::Memory;
        use opendal::Operator;

        let op = Operator::new(Memory::default()).unwrap();
        op.write(
            "components/roles/app_role.sql",
            r#"CREATE ROLE app_user WITH LOGIN PASSWORD {{ secret("application_password") }};"#,
        )
        .await
        .unwrap();

        let pather = FolderPather {
            spawn_folder: "".to_string(),
        };
        let store = Store::new(Box::new(Latest::new("").unwrap()), op.clone(), pather).unwrap();

        let gen = StreamingGeneration {
            store,
            template_contents: "BEGIN;\n\n{% include \"roles/app_role.sql\" %}\n\nCOMMIT;\n"
                .to_string(),
            template_label: "migrations/20260101000000-bootstrap/up.sql".to_string(),
            script_name: "20260101000000-bootstrap".to_string(),
            script_type: RenderKind::Migration,
            environment: "staging".to_string(),
            variables: crate::variables::Variables::default(),
            engine: EngineType::PostgresPSQL,
            secrets: Arc::new(SecretsRepository::empty(op)),
            pin_hash: None,
        };

        let mut buffer = Vec::new();
        let err = gen.render_to_writer(&mut buffer).unwrap_err();
        let chain = format!("{:?}", err);

        assert!(
            chain.contains("is not defined in spawn.toml"),
            "the underlying cause must survive the BadInclude wrapper, got: {chain}"
        );
        assert!(
            chain.contains("roles/app_role.sql:1"),
            "the cause must name the component and the line within it, got: {chain}"
        );
        assert!(
            chain.contains("migrations/20260101000000-bootstrap/up.sql:3"),
            "the outer frame must name the migration's real path and line, got: {chain}"
        );
    }

    /// Renders `template` with `vars`; returns the headline and the full chain.
    fn undefined_error_chain(template: &str, vars: serde_json::Value) -> (String, String) {
        let op = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
        let mut gen = streaming_generation(template, SecretsRepository::empty(op));
        gen.variables = crate::variables::Variables::from_str("json", &vars.to_string()).unwrap();

        let mut buffer = Vec::new();
        let err = gen
            .render_to_writer(&mut buffer)
            .expect_err("an unset variable should fail the render");

        (err.to_string(), format!("{err:?}"))
    }

    #[test]
    fn undefined_variable_error_names_the_expression() {
        let (headline, chain) = undefined_error_chain("SELECT {{ variables.tenant }};", json!({}));

        assert_eq!(headline, "undefined value in expression `variables.tenant`");
        assert!(
            chain.contains("undefined value (in migrations/test/up.sql:1)"),
            "the location must survive as the cause, got: {chain}"
        );
    }

    #[test]
    fn undefined_loop_iterable_is_named() {
        let (headline, _) =
            undefined_error_chain("{% for row in variables.rows %}x{% endfor %}", json!({}));

        assert_eq!(headline, "undefined value in expression `variables.rows`");
    }

    #[test]
    fn a_reported_expression_is_always_text_from_the_template() {
        // minijinja attributes an undefined value to the operation that raised,
        // not to the variable: these currently report `.b.c`, `upper` and `k`
        // rather than the name that was never set. That is upstream's business
        // and may improve. What spawn owes the reader either way is that the
        // quoted text genuinely appears in the template — an invented name
        // would send them hunting for code they never wrote.
        let cases = [
            (r#"SELECT {{ variables["dy" ~ "namic"] }};"#, json!({})),
            ("SELECT {{ variables.a.b.c }};", json!({"a": {"b": {}}})),
            ("SELECT {{ variables.tenant | upper }};", json!({})),
            ("{% set k = variables.missing %}SELECT {{ k }};", json!({})),
        ];

        for (template, vars) in cases {
            let (headline, _) = undefined_error_chain(template, vars);

            let expression = headline
                .strip_prefix("undefined value in expression `")
                .and_then(|rest| rest.strip_suffix('`'))
                .unwrap_or_else(|| panic!("unexpected message shape for {template}: {headline}"));

            assert!(
                !expression.is_empty() && template.contains(expression),
                "{expression:?} is not text from {template:?}"
            );
        }
    }

    #[test]
    fn multibyte_source_before_the_expression_does_not_break_the_slice() {
        // The span is a byte offset, so earlier multi-byte characters shift it.
        let (headline, _) =
            undefined_error_chain("-- 🚀 café 日本語\nSELECT {{ variables.nope }};", json!({}));

        assert_eq!(headline, "undefined value in expression `variables.nope`");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn undefined_inside_an_included_component_names_the_expression_not_the_include() {
        use crate::config::FolderPather;
        use opendal::services::Memory;
        use opendal::Operator;

        let op = Operator::new(Memory::default()).unwrap();
        op.write("components/tenant.sql", "SELECT {{ variables.tenant }};")
            .await
            .unwrap();

        let pather = FolderPather {
            spawn_folder: "".to_string(),
        };
        let store = Store::new(Box::new(Latest::new("").unwrap()), op.clone(), pather).unwrap();

        let gen = StreamingGeneration {
            store,
            template_contents: "BEGIN;\n\n{% include \"tenant.sql\" %}\n\nCOMMIT;\n".to_string(),
            template_label: "migrations/20260101000000-bootstrap/up.sql".to_string(),
            script_name: "20260101000000-bootstrap".to_string(),
            script_type: RenderKind::Migration,
            environment: "staging".to_string(),
            variables: crate::variables::Variables::default(),
            engine: EngineType::PostgresPSQL,
            secrets: Arc::new(SecretsRepository::empty(op)),
            pin_hash: None,
        };

        let mut buffer = Vec::new();
        let err = gen
            .render_to_writer(&mut buffer)
            .expect_err("an unset variable in a component should fail the render");
        let chain = format!("{err:?}");

        // The wrapper error points at the include tag, so the message has to
        // come from the inner error.
        assert_eq!(
            err.to_string(),
            "undefined value in expression `variables.tenant`",
            "expected the component's expression, not the include tag, got: {chain}"
        );
        assert!(
            chain.contains("tenant.sql:1"),
            "the cause must still name the component and line, got: {chain}"
        );
        assert!(
            chain.contains("migrations/20260101000000-bootstrap/up.sql:3"),
            "the outer frame must still name the migration and line, got: {chain}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_non_undefined_render_error_is_not_annotated_with_an_expression() {
        // Other error kinds carry their own message, which says more.
        let gen = streaming_generation(
            r#"SELECT {{ secret("nope") }};"#,
            SecretsRepository::empty(
                opendal::Operator::new(opendal::services::Memory::default()).unwrap(),
            ),
        );

        let mut buffer = Vec::new();
        let err = gen.render_to_writer(&mut buffer).unwrap_err();
        let chain = format!("{err:?}");

        assert!(
            !chain.contains("in expression"),
            "only undefined-value errors should be annotated, got: {chain}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_formatter_can_print_the_debug_snapshot_from_a_render_error() {
        // Guards `set_debug(false)` in `template_env`. Debug mode would put the
        // values in scope into every error, so this fails if it is turned back
        // on — including by deleting the line, since minijinja defaults it on
        // in dev builds.
        for template in [
            r#"{% set pw = secret("application_password") %}{{ pw }}{{ variables.missing }}"#,
            r#"{% set pw = secret("application_password") %}{{ pw }}{{ "gone.txt"|read_file }}"#,
        ] {
            let mut gen = streaming_generation(template, literal_secret_repo("hunter2"));
            gen.variables =
                crate::variables::Variables::from_str("json", r#"{"api_key":"swordfish"}"#)
                    .unwrap();

            let (script, _secrets) = gen.into_script_source();
            let mut buffer = Vec::new();
            let err = script(&mut buffer).expect_err("both templates fail to render");

            for rendered in [
                format!("{err}"),
                format!("{err:#}"),
                format!("{err:?}"),
                format!("{err:#?}"),
            ] {
                assert!(
                    !rendered.contains("hunter2"),
                    "a resolved secret reached the output: {rendered}"
                );
                assert!(
                    !rendered.contains("swordfish"),
                    "a variable value reached the output: {rendered}"
                );
                assert!(
                    !rendered.contains("Referenced variables"),
                    "minijinja's debug snapshot reached the output: {rendered}"
                );
            }
        }
    }

    #[test]
    fn a_write_failure_keeps_the_io_error_reachable_for_broken_pipe_detection() {
        // The writer's error kind is found by searching the chain, so the
        // expression must be added as a context layer rather than replacing the
        // error. Nothing else covers this.
        struct BrokenPipeWriter;

        impl std::io::Write for BrokenPipeWriter {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "psql exited first",
                ))
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let gen = streaming_generation(
            "SELECT 1;",
            SecretsRepository::empty(
                opendal::Operator::new(opendal::services::Memory::default()).unwrap(),
            ),
        );
        let (script, _secrets) = gen.into_script_source();

        let err = script(&mut BrokenPipeWriter).expect_err("the writer always fails");

        assert_eq!(
            err.kind(),
            std::io::ErrorKind::BrokenPipe,
            "the writer's error kind must survive the minijinja and anyhow wrappers"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn render_errors_are_labelled_with_the_templates_real_path() {
        let gen = streaming_generation(
            r#"SELECT {{ secret("nope") }};"#,
            SecretsRepository::empty(
                opendal::Operator::new(opendal::services::Memory::default()).unwrap(),
            ),
        );

        let mut buffer = Vec::new();
        let err = gen.render_to_writer(&mut buffer).unwrap_err();
        let chain = format!("{:?}", err);

        assert!(
            chain.contains("migrations/test/up.sql"),
            "expected the real template path in the error, got: {chain}"
        );
        assert!(
            !chain.contains("migration.sql"),
            "the hardcoded placeholder name must be gone, got: {chain}"
        );
    }
}
