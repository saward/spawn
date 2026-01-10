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

/// Recursively formats a minijinja Value for safe SQL interpolation.
///
/// This handles all ValueKind variants appropriately:
/// - Strings: Escaped using PostgreSQL literal escaping (wrapped in quotes)
/// - Numbers: Output as-is (safe for SQL)
/// - Booleans: Converted to SQL TRUE/FALSE
/// - None/Undefined: Converted to NULL/empty
/// - Bytes: Converted to PostgreSQL bytea hex format
/// - Seq/Iterable: Converted to PostgreSQL ARRAY[] with recursively escaped elements
/// - Map: Converted to JSON string (user can cast to ::jsonb)
/// - Plain: Stringified and escaped
/// - Invalid: Returns an error
fn format_value_for_sql(value: &minijinja::Value) -> Result<String, minijinja::Error> {
    match value.kind() {
        ValueKind::Undefined => Ok(String::new()),
        ValueKind::None => Ok("NULL".to_string()),
        ValueKind::Bool => {
            let b: bool = value.clone().try_into().unwrap_or(false);
            Ok(if b { "TRUE" } else { "FALSE" }.to_string())
        }
        ValueKind::Number => Ok(value.to_string()),
        ValueKind::String => {
            let s = value.to_string();
            Ok(escape_literal(&s))
        }
        ValueKind::Bytes => {
            // Convert to PostgreSQL bytea hex format: '\xDEADBEEF'::bytea
            if let Some(bytes) = value.as_bytes() {
                let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
                Ok(format!("'\\x{}'::bytea", hex))
            } else {
                Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    "Could not extract bytes from value",
                ))
            }
        }
        ValueKind::Seq | ValueKind::Iterable => {
            // Convert to PostgreSQL ARRAY[] syntax with recursively escaped elements
            // e.g., [1, 'hello', true] becomes ARRAY[1, 'hello', TRUE]
            let mut elements = Vec::new();
            for item in value.try_iter().map_err(|e| {
                minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    format!("Could not iterate over sequence: {}", e),
                )
            })? {
                elements.push(format_value_for_sql(&item)?);
            }
            Ok(format!("ARRAY[{}]", elements.join(", ")))
        }
        ValueKind::Map => {
            // Maps don't have a native SQL representation.
            // Convert to JSON-like string representation and escape it.
            // Users can cast to ::jsonb if needed: {{ my_map }}::jsonb
            let s = value.to_string();
            Ok(escape_literal(&s))
        }
        ValueKind::Plain => {
            // For custom objects, stringify and escape as a string
            let s = value.to_string();
            Ok(escape_literal(&s))
        }
        ValueKind::Invalid => {
            // Invalid values contain errors - propagate them
            Err(minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                format!("Invalid value encountered in SQL template: {}", value),
            ))
        }
        // ValueKind is non-exhaustive, handle any future variants safely
        _ => {
            // For unknown types, stringify and escape as a string (safe default)
            let s = value.to_string();
            Ok(escape_literal(&s))
        }
    }
}

/// Custom formatter that escapes values for safe SQL interpolation.
///
/// This formatter is invoked when auto-escape is enabled (for .sql templates).
/// It delegates to format_value_for_sql for type-specific handling.
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

        let formatted = format_value_for_sql(value)?;
        write!(out, "{}", formatted)
            .map_err(|e| minijinja::Error::new(minijinja::ErrorKind::WriteFailure, e.to_string()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::Value;

    /// Helper to test SQL formatting of a value by rendering it in a .sql template
    fn render_sql_value(value: Value) -> String {
        let mut env = Environment::new();
        env.set_auto_escape_callback(|name| {
            if name.ends_with(".sql") {
                AutoEscape::Custom("sql")
            } else {
                AutoEscape::None
            }
        });
        env.set_formatter(sql_escape_formatter);
        env.add_template("test.sql", "{{ value }}").unwrap();
        let tmpl = env.get_template("test.sql").unwrap();
        tmpl.render(context!(value => value)).unwrap()
    }

    #[test]
    fn test_sql_escape_string() {
        let result = render_sql_value(Value::from("hello"));
        assert_eq!(result, "'hello'");
    }

    #[test]
    fn test_sql_escape_string_with_quotes() {
        let result = render_sql_value(Value::from("it's a test"));
        assert_eq!(result, "'it''s a test'");
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
    fn test_sql_escape_negative_integer() {
        let result = render_sql_value(Value::from(-123));
        assert_eq!(result, "-123");
    }

    #[test]
    fn test_sql_escape_float() {
        let result = render_sql_value(Value::from(3.14));
        assert_eq!(result, "3.14");
    }

    #[test]
    fn test_sql_escape_bool_true() {
        let result = render_sql_value(Value::from(true));
        assert_eq!(result, "TRUE");
    }

    #[test]
    fn test_sql_escape_bool_false() {
        let result = render_sql_value(Value::from(false));
        assert_eq!(result, "FALSE");
    }

    #[test]
    fn test_sql_escape_none() {
        let result = render_sql_value(Value::from(()));
        assert_eq!(result, "NULL");
    }

    #[test]
    fn test_sql_escape_undefined() {
        let result = render_sql_value(Value::UNDEFINED);
        assert_eq!(result, "");
    }

    #[test]
    fn test_sql_escape_seq_integers() {
        let result = render_sql_value(Value::from(vec![1, 2, 3]));
        assert_eq!(result, "ARRAY[1, 2, 3]");
    }

    #[test]
    fn test_sql_escape_seq_strings() {
        let result = render_sql_value(Value::from(vec!["hello", "world"]));
        assert_eq!(result, "ARRAY['hello', 'world']");
    }

    #[test]
    fn test_sql_escape_seq_strings_with_injection() {
        let result = render_sql_value(Value::from(vec!["safe", "'; DROP TABLE users; --"]));
        assert_eq!(result, "ARRAY['safe', '''; DROP TABLE users; --']");
    }

    #[test]
    fn test_sql_escape_seq_mixed_types() {
        // Create a mixed-type sequence using Value::from_iter
        let values: Vec<Value> = vec![Value::from(1), Value::from("hello"), Value::from(true)];
        let result = render_sql_value(Value::from(values));
        assert_eq!(result, "ARRAY[1, 'hello', TRUE]");
    }

    #[test]
    fn test_sql_escape_seq_empty() {
        let empty: Vec<i32> = vec![];
        let result = render_sql_value(Value::from(empty));
        assert_eq!(result, "ARRAY[]");
    }

    #[test]
    fn test_sql_escape_nested_seq() {
        // Nested arrays: [[1, 2], [3, 4]]
        let inner1 = Value::from(vec![1, 2]);
        let inner2 = Value::from(vec![3, 4]);
        let outer = Value::from(vec![inner1, inner2]);
        let result = render_sql_value(outer);
        assert_eq!(result, "ARRAY[ARRAY[1, 2], ARRAY[3, 4]]");
    }

    #[test]
    fn test_sql_escape_map() {
        // Maps get converted to JSON-like string and escaped
        use std::collections::BTreeMap;
        let mut map = BTreeMap::new();
        map.insert("name", "Alice");
        map.insert("role", "admin");
        let result = render_sql_value(Value::from(map));
        // The map will be stringified and escaped as a SQL literal
        // Exact format depends on minijinja's Display impl for maps
        assert!(result.starts_with("'"));
        assert!(result.ends_with("'"));
        assert!(result.contains("name"));
        assert!(result.contains("Alice"));
    }

    #[test]
    fn test_sql_escape_bytes() {
        let bytes = Value::from_bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        let result = render_sql_value(bytes);
        assert_eq!(result, "'\\xdeadbeef'::bytea");
    }

    #[test]
    fn test_sql_escape_bytes_empty() {
        let bytes = Value::from_bytes(vec![]);
        let result = render_sql_value(bytes);
        assert_eq!(result, "'\\x'::bytea");
    }

    #[test]
    fn test_sql_no_escape_for_non_sql_templates() {
        let mut env = Environment::new();
        env.set_auto_escape_callback(|name| {
            if name.ends_with(".sql") {
                AutoEscape::Custom("sql")
            } else {
                AutoEscape::None
            }
        });
        env.set_formatter(sql_escape_formatter);
        // Use .txt extension - should NOT trigger SQL escaping
        env.add_template("test.txt", "{{ value }}").unwrap();
        let tmpl = env.get_template("test.txt").unwrap();
        let result = tmpl.render(context!(value => "hello")).unwrap();
        // No quotes, no escaping
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_sql_safe_filter_bypasses_escaping() {
        let mut env = Environment::new();
        env.set_auto_escape_callback(|name| {
            if name.ends_with(".sql") {
                AutoEscape::Custom("sql")
            } else {
                AutoEscape::None
            }
        });
        env.set_formatter(sql_escape_formatter);
        // Using |safe filter should bypass escaping
        env.add_template("test.sql", "{{ value|safe }}").unwrap();
        let tmpl = env.get_template("test.sql").unwrap();
        let result = tmpl.render(context!(value => "raw SQL here")).unwrap();
        // Should be output as-is without quotes
        assert_eq!(result, "raw SQL here");
    }

    #[test]
    fn test_sql_escape_only_on_output_not_in_loops() {
        // Verify that the formatter only applies when outputting values,
        // not when iterating over sequences in loops
        let mut env = Environment::new();
        env.set_auto_escape_callback(|name| {
            if name.ends_with(".sql") {
                AutoEscape::Custom("sql")
            } else {
                AutoEscape::None
            }
        });
        env.set_formatter(sql_escape_formatter);

        // Template that loops over a sequence and outputs each item
        let template =
            r#"{% for item in items %}{{ item }}{% if not loop.last %}, {% endif %}{% endfor %}"#;
        env.add_template("test.sql", template).unwrap();
        let tmpl = env.get_template("test.sql").unwrap();

        let items = vec!["alice", "bob", "charlie"];
        let result = tmpl.render(context!(items => items)).unwrap();

        // Each item should be escaped individually when output
        assert_eq!(result, "'alice', 'bob', 'charlie'");
    }

    #[test]
    fn test_sql_escape_only_on_output_not_in_conditionals() {
        // Verify that conditionals work with unescaped values
        let mut env = Environment::new();
        env.set_auto_escape_callback(|name| {
            if name.ends_with(".sql") {
                AutoEscape::Custom("sql")
            } else {
                AutoEscape::None
            }
        });
        env.set_formatter(sql_escape_formatter);

        let template = r#"{% if enabled %}{{ value }}{% else %}NULL{% endif %}"#;
        env.add_template("test.sql", template).unwrap();
        let tmpl = env.get_template("test.sql").unwrap();

        // Test with enabled=true
        let result = tmpl
            .render(context!(enabled => true, value => "test"))
            .unwrap();
        assert_eq!(result, "'test'");

        // Test with enabled=false
        let result = tmpl
            .render(context!(enabled => false, value => "test"))
            .unwrap();
        assert_eq!(result, "NULL");
    }

    #[test]
    fn test_sql_escape_loop_over_map_keys() {
        // Verify that we can loop over map keys and values
        let mut env = Environment::new();
        env.set_auto_escape_callback(|name| {
            if name.ends_with(".sql") {
                AutoEscape::Custom("sql")
            } else {
                AutoEscape::None
            }
        });
        env.set_formatter(sql_escape_formatter);

        // Template that loops over a map using items() method
        let template = r#"{% for key, val in data|items %}{{ key }} = {{ val }}{% if not loop.last %}, {% endif %}{% endfor %}"#;
        env.add_template("test.sql", template).unwrap();
        let tmpl = env.get_template("test.sql").unwrap();

        use std::collections::BTreeMap;
        let mut data = BTreeMap::new();
        data.insert("name", "Alice");
        data.insert("role", "admin");

        let result = tmpl.render(context!(data => data)).unwrap();
        // Keys and values should both be escaped when output
        assert_eq!(result, "'name' = 'Alice', 'role' = 'admin'");
    }

    #[test]
    fn test_sql_escape_nested_loop() {
        // Verify nested loops work correctly
        let mut env = Environment::new();
        env.set_auto_escape_callback(|name| {
            if name.ends_with(".sql") {
                AutoEscape::Custom("sql")
            } else {
                AutoEscape::None
            }
        });
        env.set_formatter(sql_escape_formatter);

        let template = r#"{% for row in rows %}({% for col in row %}{{ col }}{% if not loop.last %}, {% endif %}{% endfor %}){% if not loop.last %}, {% endif %}{% endfor %}"#;
        env.add_template("test.sql", template).unwrap();
        let tmpl = env.get_template("test.sql").unwrap();

        let rows: Vec<Vec<&str>> = vec![vec!["a", "b"], vec!["c", "d"]];
        let result = tmpl.render(context!(rows => rows)).unwrap();
        assert_eq!(result, "('a', 'b'), ('c', 'd')");
    }

    #[test]
    fn test_sql_escape_filter_fails_for_custom_format() {
        // GOOD NEWS: The |escape filter in minijinja does NOT work with custom
        // auto-escape formats like our "sql" format. It throws an error:
        // "Default formatter does not know how to format to custom format 'sql'"
        //
        // This means users can't accidentally use |escape thinking it does SQL escaping.
        // Only the |safe filter bypasses our escaping, which is intentional.
        let mut env = Environment::new();
        env.set_auto_escape_callback(|name| {
            if name.ends_with(".sql") {
                AutoEscape::Custom("sql")
            } else {
                AutoEscape::None
            }
        });
        env.set_formatter(sql_escape_formatter);

        env.add_template("test.sql", "{{ value|escape }}").unwrap();
        let tmpl = env.get_template("test.sql").unwrap();

        // The |escape filter should fail for custom SQL format
        let result = tmpl.render(context!(value => "test"));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("does not know how to format to custom format"));
    }

    #[test]
    fn test_sql_escape_from_safe_string_bypasses_escaping() {
        // SECURITY NOTE: Value::from_safe_string() in Rust code bypasses SQL escaping.
        // This is the same mechanism that |safe uses, but done from Rust code.
        //
        // This is NOT a vulnerability if you control the Rust code, but be careful
        // not to wrap user-controlled input in from_safe_string().
        let mut env = Environment::new();
        env.set_auto_escape_callback(|name| {
            if name.ends_with(".sql") {
                AutoEscape::Custom("sql")
            } else {
                AutoEscape::None
            }
        });
        env.set_formatter(sql_escape_formatter);

        env.add_template("test.sql", "{{ value }}").unwrap();
        let tmpl = env.get_template("test.sql").unwrap();

        // Creating a safe string from Rust code bypasses escaping
        let safe_value = Value::from_safe_string("1 OR 1=1".to_string());
        let result = tmpl.render(context!(value => safe_value)).unwrap();

        // This outputs raw SQL - the value is trusted because Rust code marked it safe
        assert_eq!(result, "1 OR 1=1");

        // Compare with normal string which gets escaped
        let normal_value = Value::from("1 OR 1=1");
        let result = tmpl.render(context!(value => normal_value)).unwrap();
        assert_eq!(result, "'1 OR 1=1'");
    }

    #[test]
    fn test_sql_escape_length_filter_works() {
        // Verify that filters like |length work on unescaped values
        let mut env = Environment::new();
        env.set_auto_escape_callback(|name| {
            if name.ends_with(".sql") {
                AutoEscape::Custom("sql")
            } else {
                AutoEscape::None
            }
        });
        env.set_formatter(sql_escape_formatter);

        let template = r#"{% if items|length > 0 %}{{ items|length }}{% else %}0{% endif %}"#;
        env.add_template("test.sql", template).unwrap();
        let tmpl = env.get_template("test.sql").unwrap();

        let items = vec![1, 2, 3];
        let result = tmpl.render(context!(items => items)).unwrap();
        // |length returns a number, which is output directly
        assert_eq!(result, "3");
    }
}
