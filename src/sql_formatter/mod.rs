//! SQL escaping formatters for minijinja templates.
//!
//! This crate provides SQL-safe value formatting for different database dialects.
//! Each dialect module implements escaping rules appropriate for that database.
//!
//! # Supported Dialects
//!
//! - [`SqlDialect::Postgres`] - PostgreSQL escaping (works for psql CLI and native drivers)
//!
//! # Usage
//!
//! ```
//! use spawn::sql_formatter::{SqlDialect, get_auto_escape_callback, get_formatter};
//! use minijinja::Environment;
//!
//! let mut env = Environment::new();
//! env.set_auto_escape_callback(get_auto_escape_callback(SqlDialect::Postgres));
//! env.set_formatter(get_formatter(SqlDialect::Postgres));
//! ```

pub mod postgres;

use minijinja::{AutoEscape, Output, State, Value};

/// SQL dialect for formatting.
///
/// Different databases have different escaping rules and syntax for literals,
/// identifiers, arrays, etc. This enum selects the appropriate formatter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlDialect {
    /// PostgreSQL dialect.
    ///
    /// Used for:
    /// - `psql` CLI tool
    /// - Native PostgreSQL drivers (e.g., `tokio-postgres`, `sqlx` with postgres)
    /// - PostgreSQL-compatible databases (e.g., CockroachDB, YugabyteDB)
    Postgres,
    // Future dialects:
    // MySQL,
    // SqlServer,
    // Sqlite,
}

impl SqlDialect {
    /// Returns the auto-escape format name for this dialect.
    ///
    /// This is used with minijinja's `AutoEscape::Custom` to identify
    /// which escaping mode is active.
    pub fn format_name(&self) -> &'static str {
        match self {
            SqlDialect::Postgres => "sql-postgres",
        }
    }
}

/// Type alias for minijinja formatter functions.
pub type FormatterFn = fn(&mut Output<'_>, &State<'_, '_>, &Value) -> Result<(), minijinja::Error>;

/// Type alias for minijinja auto-escape callback functions.
pub type AutoEscapeCallback = fn(&str) -> AutoEscape;

/// Returns the appropriate formatter function for the given SQL dialect.
///
/// The returned function can be passed directly to `Environment::set_formatter()`.
///
/// # Example
///
/// ```
/// use spawn::sql_formatter::{SqlDialect, get_formatter};
/// use minijinja::Environment;
///
/// let mut env = Environment::new();
/// let formatter = get_formatter(SqlDialect::Postgres);
/// env.set_formatter(formatter);
/// ```
pub fn get_formatter(dialect: SqlDialect) -> FormatterFn {
    match dialect {
        SqlDialect::Postgres => postgres::sql_escape_formatter,
    }
}

/// Returns an auto-escape callback for the given SQL dialect.
///
/// The callback enables SQL escaping for all files.
/// This should be passed to `Environment::set_auto_escape_callback()`.
///
/// # Example
///
/// ```
/// use spawn::sql_formatter::{SqlDialect, get_auto_escape_callback};
/// use minijinja::Environment;
///
/// let mut env = Environment::new();
/// let callback = get_auto_escape_callback(SqlDialect::Postgres);
/// env.set_auto_escape_callback(callback);
/// ```
pub fn get_auto_escape_callback(dialect: SqlDialect) -> AutoEscapeCallback {
    match dialect {
        SqlDialect::Postgres => postgres::auto_escape_callback,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dialect_format_names_are_unique() {
        // Ensure each dialect has a unique format name
        let dialects = [SqlDialect::Postgres];
        let names: Vec<_> = dialects.iter().map(|d| d.format_name()).collect();

        for (i, name) in names.iter().enumerate() {
            for (j, other) in names.iter().enumerate() {
                if i != j {
                    assert_ne!(name, other, "Dialect format names must be unique");
                }
            }
        }
    }

    #[test]
    fn test_get_formatter_returns_function() {
        // Just verify we can get a formatter for each dialect
        let _ = get_formatter(SqlDialect::Postgres);
    }

    #[test]
    fn test_get_auto_escape_callback_returns_function() {
        let callback = get_auto_escape_callback(SqlDialect::Postgres);

        // Verify .sql files trigger custom escaping
        match callback("test.sql") {
            AutoEscape::Custom(name) => assert_eq!(name, "sql-postgres"),
            _ => panic!("Expected Custom auto-escape for .sql files"),
        }

        // Verify non-.sql files also trigger escaping (all files use SQL escaping)
        match callback("test.txt") {
            AutoEscape::Custom(name) => assert_eq!(name, "sql-postgres"),
            _ => panic!("Expected Custom auto-escape for .txt files"),
        }
    }
}
