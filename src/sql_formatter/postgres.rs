//! PostgreSQL-specific SQL escaping for minijinja templates.
//!
//! This module provides safe SQL value formatting for PostgreSQL databases.
//! It handles all minijinja value types and converts them to appropriate
//! PostgreSQL literal syntax.
//!
//! # Escaping Rules
//!
//! - **Strings**: Escaped using PostgreSQL's `escape_literal` (handles quotes, special chars)
//! - **Numbers**: Output directly (integers and floats are safe)
//! - **Booleans**: Converted to `TRUE` / `FALSE`
//! - **None**: Converted to `NULL`
//! - **Undefined**: Empty string (consistent with minijinja defaults)
//! - **Bytes**: Converted to PostgreSQL bytea hex format (`'\xDEADBEEF'::bytea`)
//! - **Sequences**: Converted to PostgreSQL `ARRAY[...]` with recursively escaped elements
//! - **Maps**: Converted to JSON-like string and escaped (can be cast to `::jsonb`)
//! - **Plain objects**: Stringified and escaped
//! - **Invalid values**: Return an error
//!
//! # Security
//!
//! The only ways to bypass escaping are:
//! - Using the `|safe` filter in templates (intentional)
//! - Using `Value::from_safe_string()` in Rust code (requires explicit code)
//!
//! The `|escape` filter will error for custom SQL formats, preventing accidental misuse.

use minijinja::value::ValueKind;
use minijinja::{AutoEscape, Output, State, Value};
use postgres_protocol::escape::escape_literal;

/// The auto-escape format name for PostgreSQL.
pub const FORMAT_NAME: &str = "sql-postgres";

/// Auto-escape callback for PostgreSQL SQL templates.
///
/// Enables SQL escaping for all files.
pub fn auto_escape_callback(_name: &str) -> AutoEscape {
    AutoEscape::Custom(FORMAT_NAME)
}

/// Recursively formats a minijinja Value for safe PostgreSQL interpolation.
///
/// This handles all ValueKind variants appropriately for PostgreSQL syntax.
fn format_value_for_postgres(value: &Value) -> Result<String, minijinja::Error> {
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
                elements.push(format_value_for_postgres(&item)?);
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

/// Custom formatter that escapes values for safe PostgreSQL interpolation.
///
/// This formatter is invoked when auto-escape is enabled (for .sql templates).
/// It delegates to `format_value_for_postgres` for type-specific handling.
///
/// # Bypass Mechanisms
///
/// - Values marked as safe (via `|safe` filter) are output without escaping
/// - Only applies when `state.auto_escape()` matches our custom format
pub fn sql_escape_formatter(
    out: &mut Output<'_>,
    state: &State<'_, '_>,
    value: &Value,
) -> Result<(), minijinja::Error> {
    // Check if we're in PostgreSQL SQL auto-escape mode
    if state.auto_escape() == AutoEscape::Custom(FORMAT_NAME) {
        // If the value is marked as safe (via |safe filter), skip escaping
        if value.is_safe() {
            return write!(out, "{}", value).map_err(|e| {
                minijinja::Error::new(minijinja::ErrorKind::WriteFailure, e.to_string())
            });
        }

        let formatted = format_value_for_postgres(value)?;
        write!(out, "{}", formatted)
            .map_err(|e| minijinja::Error::new(minijinja::ErrorKind::WriteFailure, e.to_string()))
    } else {
        // For non-SQL templates, use default formatting (no escaping)
        write!(out, "{}", value)
            .map_err(|e| minijinja::Error::new(minijinja::ErrorKind::WriteFailure, e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::{context, Environment};

    /// Helper to test SQL formatting of a value by rendering it in a .sql template
    fn render_sql_value(value: Value) -> String {
        let mut env = Environment::new();
        env.set_auto_escape_callback(auto_escape_callback);
        env.set_formatter(sql_escape_formatter);
        env.add_template("test.sql", "{{ value }}").unwrap();
        let tmpl = env.get_template("test.sql").unwrap();
        tmpl.render(context!(value => value)).unwrap()
    }

    // ===================
    // String escaping tests
    // ===================

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

    // ===================
    // Number tests
    // ===================

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

    // ===================
    // Boolean tests
    // ===================

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

    // ===================
    // None/Undefined tests
    // ===================

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

    // ===================
    // Sequence/Array tests
    // ===================

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
        let inner1 = Value::from(vec![1, 2]);
        let inner2 = Value::from(vec![3, 4]);
        let outer = Value::from(vec![inner1, inner2]);
        let result = render_sql_value(outer);
        assert_eq!(result, "ARRAY[ARRAY[1, 2], ARRAY[3, 4]]");
    }

    // ===================
    // Map tests
    // ===================

    #[test]
    fn test_sql_escape_map() {
        use std::collections::BTreeMap;
        let mut map = BTreeMap::new();
        map.insert("name", "Alice");
        map.insert("role", "admin");
        let result = render_sql_value(Value::from(map));
        // The map will be stringified and escaped as a SQL literal
        assert!(result.starts_with("'"));
        assert!(result.ends_with("'"));
        assert!(result.contains("name"));
        assert!(result.contains("Alice"));
    }

    // ===================
    // Bytes tests
    // ===================

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

    // ===================
    // Auto-escape behavior tests
    // ===================

    #[test]
    fn test_sql_escape_for_non_sql_templates() {
        let mut env = Environment::new();
        env.set_auto_escape_callback(auto_escape_callback);
        env.set_formatter(sql_escape_formatter);
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
        env.set_auto_escape_callback(auto_escape_callback);
        env.set_formatter(sql_escape_formatter);
        // Using |safe filter should bypass escaping
        env.add_template("test.sql", "{{ value|safe }}").unwrap();
        let tmpl = env.get_template("test.sql").unwrap();
        let result = tmpl.render(context!(value => "raw SQL here")).unwrap();
        // Should be output as-is without quotes
        assert_eq!(result, "raw SQL here");
    }

    #[test]
    fn test_sql_escape_filter_fails_for_custom_format() {
        // The |escape filter in minijinja does NOT work with custom formats
        let mut env = Environment::new();
        env.set_auto_escape_callback(auto_escape_callback);
        env.set_formatter(sql_escape_formatter);

        env.add_template("test.sql", "{{ value|escape }}").unwrap();
        let tmpl = env.get_template("test.sql").unwrap();

        let result = tmpl.render(context!(value => "test"));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("does not know how to format to custom format"));
    }

    #[test]
    fn test_sql_escape_from_safe_string_bypasses_escaping() {
        // Value::from_safe_string() bypasses SQL escaping
        let mut env = Environment::new();
        env.set_auto_escape_callback(auto_escape_callback);
        env.set_formatter(sql_escape_formatter);

        env.add_template("test.sql", "{{ value }}").unwrap();
        let tmpl = env.get_template("test.sql").unwrap();

        let safe_value = Value::from_safe_string("1 OR 1=1".to_string());
        let result = tmpl.render(context!(value => safe_value)).unwrap();
        assert_eq!(result, "1 OR 1=1");

        // Compare with normal string which gets escaped
        let normal_value = Value::from("1 OR 1=1");
        let result = tmpl.render(context!(value => normal_value)).unwrap();
        assert_eq!(result, "'1 OR 1=1'");
    }

    // ===================
    // Loop/conditional tests (verify formatter only applies to output)
    // ===================

    #[test]
    fn test_sql_escape_only_on_output_not_in_loops() {
        let mut env = Environment::new();
        env.set_auto_escape_callback(auto_escape_callback);
        env.set_formatter(sql_escape_formatter);

        let template =
            r#"{% for item in items %}{{ item }}{% if not loop.last %}, {% endif %}{% endfor %}"#;
        env.add_template("test.sql", template).unwrap();
        let tmpl = env.get_template("test.sql").unwrap();

        let items = vec!["alice", "bob", "charlie"];
        let result = tmpl.render(context!(items => items)).unwrap();
        assert_eq!(result, "'alice', 'bob', 'charlie'");
    }

    #[test]
    fn test_sql_escape_only_on_output_not_in_conditionals() {
        let mut env = Environment::new();
        env.set_auto_escape_callback(auto_escape_callback);
        env.set_formatter(sql_escape_formatter);

        let template = r#"{% if enabled %}{{ value }}{% else %}NULL{% endif %}"#;
        env.add_template("test.sql", template).unwrap();
        let tmpl = env.get_template("test.sql").unwrap();

        let result = tmpl
            .render(context!(enabled => true, value => "test"))
            .unwrap();
        assert_eq!(result, "'test'");

        let result = tmpl
            .render(context!(enabled => false, value => "test"))
            .unwrap();
        assert_eq!(result, "NULL");
    }

    #[test]
    fn test_sql_escape_loop_over_map_keys() {
        let mut env = Environment::new();
        env.set_auto_escape_callback(auto_escape_callback);
        env.set_formatter(sql_escape_formatter);

        let template = r#"{% for key, val in data|items %}{{ key }} = {{ val }}{% if not loop.last %}, {% endif %}{% endfor %}"#;
        env.add_template("test.sql", template).unwrap();
        let tmpl = env.get_template("test.sql").unwrap();

        use std::collections::BTreeMap;
        let mut data = BTreeMap::new();
        data.insert("name", "Alice");
        data.insert("role", "admin");

        let result = tmpl.render(context!(data => data)).unwrap();
        assert_eq!(result, "'name' = 'Alice', 'role' = 'admin'");
    }

    #[test]
    fn test_sql_escape_nested_loop() {
        let mut env = Environment::new();
        env.set_auto_escape_callback(auto_escape_callback);
        env.set_formatter(sql_escape_formatter);

        let template = r#"{% for row in rows %}({% for col in row %}{{ col }}{% if not loop.last %}, {% endif %}{% endfor %}){% if not loop.last %}, {% endif %}{% endfor %}"#;
        env.add_template("test.sql", template).unwrap();
        let tmpl = env.get_template("test.sql").unwrap();

        let rows: Vec<Vec<&str>> = vec![vec!["a", "b"], vec!["c", "d"]];
        let result = tmpl.render(context!(rows => rows)).unwrap();
        assert_eq!(result, "('a', 'b'), ('c', 'd')");
    }

    #[test]
    fn test_sql_escape_length_filter_works() {
        let mut env = Environment::new();
        env.set_auto_escape_callback(auto_escape_callback);
        env.set_formatter(sql_escape_formatter);

        let template = r#"{% if items|length > 0 %}{{ items|length }}{% else %}0{% endif %}"#;
        env.add_template("test.sql", template).unwrap();
        let tmpl = env.get_template("test.sql").unwrap();

        let items = vec![1, 2, 3];
        let result = tmpl.render(context!(items => items)).unwrap();
        assert_eq!(result, "3");
    }

    // ===================
    // Auto-escape callback tests
    // ===================

    #[test]
    fn test_auto_escape_callback_all_files() {
        // SQL escaping is enabled for all file types
        assert_eq!(
            auto_escape_callback("migration.sql"),
            AutoEscape::Custom(FORMAT_NAME)
        );
        assert_eq!(
            auto_escape_callback("path/to/file.sql"),
            AutoEscape::Custom(FORMAT_NAME)
        );
        assert_eq!(
            auto_escape_callback("file.txt"),
            AutoEscape::Custom(FORMAT_NAME)
        );
        assert_eq!(
            auto_escape_callback("file.html"),
            AutoEscape::Custom(FORMAT_NAME)
        );
    }
}
