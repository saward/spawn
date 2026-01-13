//! Type-safe SQL escaping for PostgreSQL.
//!
//! This module provides wrapper types that guarantee SQL values have been properly
//! escaped at construction time. By using these types instead of raw strings,
//! the type system ensures that escaping cannot be forgotten.
//!
//! # Example
//!
//! ```
//! use spawn::{sql_query, escape::{EscapedIdentifier, EscapedLiteral}};
//!
//! let schema = EscapedIdentifier::new("my_schema");
//! let value = EscapedLiteral::new("user's input");
//!
//! let query = sql_query!(
//!     "SELECT * FROM {}.users WHERE name = {}",
//!     schema,
//!     value
//! );
//! ```

use postgres_protocol::escape::{escape_identifier, escape_literal};
use std::fmt;

/// A trait for types that are safe to interpolate into SQL queries.
///
/// Types implementing this trait can be used with the `sql_query!` macro.
/// The built-in implementations are `EscapedIdentifier`, `EscapedLiteral`,
/// and `InsecureRawSql`.
///
/// You may implement this trait for your own types if you have other
/// validated/escaped SQL fragments, but do so with caution.
pub trait SqlSafe {
    /// Returns the SQL-safe string representation.
    fn as_sql(&self) -> &str;
}

impl<S: SqlSafe> SqlSafe for Option<S> {
    fn as_sql(&self) -> &str {
        if let Some(inner) = self {
            return inner.as_sql();
        }
        "NULL"
    }
}

impl<S: SqlSafe> SqlSafe for &S {
    fn as_sql(&self) -> &str {
        (*self).as_sql()
    }
}

/// A PostgreSQL identifier (schema, table, column name) that has been safely escaped.
///
/// The value is escaped at construction time using PostgreSQL's `quote_ident` rules:
/// - Wrapped in double quotes
/// - Any embedded double quotes are doubled
///
/// # Example
///
/// ```
/// use spawn::escape::EscapedIdentifier;
///
/// let schema = EscapedIdentifier::new("my_schema");
/// assert_eq!(schema.as_str(), "\"my_schema\"");
///
/// let tricky = EscapedIdentifier::new("schema\"name");
/// assert_eq!(tricky.as_str(), "\"schema\"\"name\"");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EscapedIdentifier(String);

impl EscapedIdentifier {
    /// Creates a new escaped identifier from a raw string.
    ///
    /// The input is immediately escaped using PostgreSQL's identifier escaping rules.
    pub fn new(raw: &str) -> Self {
        Self(escape_identifier(raw))
    }

    /// Returns the escaped identifier as a string slice.
    ///
    /// This value is safe to interpolate directly into SQL queries.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EscapedIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl SqlSafe for EscapedIdentifier {
    fn as_sql(&self) -> &str {
        self.as_str()
    }
}

/// A PostgreSQL string literal that has been safely escaped.
///
/// The value is escaped at construction time using PostgreSQL's `quote_literal` rules:
/// - Wrapped in single quotes
/// - Any embedded single quotes are doubled
/// - If backslashes are present, prefixed with `E` and backslashes are doubled
///
/// # Example
///
/// ```
/// use spawn::escape::EscapedLiteral;
///
/// let value = EscapedLiteral::new("hello");
/// assert_eq!(value.as_str(), "'hello'");
///
/// let quoted = EscapedLiteral::new("it's");
/// assert_eq!(quoted.as_str(), "'it''s'");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EscapedLiteral(String);

impl EscapedLiteral {
    /// Creates a new escaped literal from a raw string.
    ///
    /// The input is immediately escaped using PostgreSQL's literal escaping rules.
    pub fn new(raw: &str) -> Self {
        Self(escape_literal(raw))
    }

    /// Returns the escaped literal as a string slice.
    ///
    /// This value is safe to interpolate directly into SQL queries.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EscapedLiteral {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl SqlSafe for EscapedLiteral {
    fn as_sql(&self) -> &str {
        self.as_str()
    }
}

/// Raw SQL that has not been escaped.
///
/// This type is for cases where you genuinely need to include raw SQL that cannot
/// be escaped, such as SQL keywords, operators, or pre-validated static strings.
///
/// # Warning
///
/// Use this type with extreme caution. It bypasses all escaping protections.
/// Only use it for:
/// - Static SQL fragments known at compile time
/// - SQL that has been validated through other means
///
/// # Example
///
/// ```
/// use spawn::{sql_query, escape::{EscapedIdentifier, InsecureRawSql}};
///
/// let schema = EscapedIdentifier::new("my_schema");
/// let order = InsecureRawSql::new("ORDER BY created_at DESC");
///
/// let query = sql_query!(
///     "SELECT * FROM {}.users {}",
///     schema,
///     order
/// );
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InsecureRawSql(String);

impl InsecureRawSql {
    /// Creates a new raw SQL fragment.
    ///
    /// # Warning
    ///
    /// This does NOT escape the input. Only use this for SQL that you have
    /// verified is safe, such as static strings or validated input.
    pub fn new(raw: &str) -> Self {
        Self(raw.to_string())
    }

    /// Returns the raw SQL as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InsecureRawSql {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl SqlSafe for InsecureRawSql {
    fn as_sql(&self) -> &str {
        self.as_str()
    }
}

/// A complete SQL query that has been constructed using only safe components.
///
/// This type can only be created through the `sql_query!` macro, which ensures
/// that all interpolated values implement `SqlSafe`.
///
/// # Example
///
/// ```
/// use spawn::{sql_query, escape::{EscapedIdentifier, EscapedLiteral}};
///
/// let schema = EscapedIdentifier::new("public");
/// let name = EscapedLiteral::new("Alice");
///
/// let query = sql_query!(
///     "SELECT * FROM {}.users WHERE name = {}",
///     schema,
///     name
/// );
///
/// assert!(query.as_str().contains("\"public\""));
/// assert!(query.as_str().contains("'Alice'"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EscapedQuery(String);

impl EscapedQuery {
    /// Creates a new EscapedQuery.
    ///
    /// This is intentionally private to the crate. Use the `sql_query!` macro instead.
    #[doc(hidden)]
    pub fn __new_from_macro(sql: String) -> Self {
        Self(sql)
    }

    /// Returns the query as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EscapedQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Creates an `EscapedQuery` from a format string and SQL-safe arguments.
///
/// This macro works like `format!`, but only accepts arguments that implement
/// the `SqlSafe` trait. This ensures that all interpolated values have been
/// properly escaped.
///
/// # Accepted Types
///
/// - `EscapedIdentifier` - for schema, table, and column names
/// - `EscapedLiteral` - for string values
/// - `InsecureRawSql` - for raw SQL (use with caution)
///
/// # Example
///
/// ```
/// use spawn::{sql_query, escape::{EscapedIdentifier, EscapedLiteral}};
///
/// let schema = EscapedIdentifier::new("my_schema");
/// let table = EscapedIdentifier::new("users");
/// let name = EscapedLiteral::new("O'Brien");
///
/// let query = sql_query!(
///     "SELECT * FROM {}.{} WHERE name = {}",
///     schema,
///     table,
///     name
/// );
/// ```
///
/// # Compile-Time Safety
///
/// Passing a raw `String` or `&str` will result in a compile error:
///
/// ```compile_fail
/// use spawn::sql_query;
///
/// let unsafe_input = "Robert'; DROP TABLE users; --";
/// let query = sql_query!("SELECT * FROM users WHERE name = {}", unsafe_input);
/// // Error: the trait bound `&str: SqlSafe` is not satisfied
/// ```
#[macro_export]
macro_rules! sql_query {
    ($fmt:literal $(, $arg:expr)* $(,)?) => {{
        $crate::escape::EscapedQuery::__new_from_macro(
            format!($fmt $(, $crate::escape::SqlSafe::as_sql(&$arg))*)
        )
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escaped_identifier_basic() {
        let ident = EscapedIdentifier::new("my_schema");
        assert_eq!(ident.as_str(), "\"my_schema\"");
    }

    #[test]
    fn test_escaped_literal_basic() {
        let lit = EscapedLiteral::new("hello");
        assert_eq!(lit.as_str(), "'hello'");
    }

    #[test]
    fn test_sql_query_macro() {
        let schema = EscapedIdentifier::new("public");
        let name = EscapedLiteral::new("Alice");

        let query = sql_query!("SELECT * FROM {}.users WHERE name = {}", schema, name);

        assert_eq!(
            query.as_str(),
            "SELECT * FROM \"public\".users WHERE name = 'Alice'"
        );
    }

    #[test]
    fn test_sql_query_with_insecure_raw() {
        let schema = EscapedIdentifier::new("public");
        let order = InsecureRawSql::new("ORDER BY id DESC");

        let query = sql_query!("SELECT * FROM {}.users {}", schema, order);

        assert_eq!(
            query.as_str(),
            "SELECT * FROM \"public\".users ORDER BY id DESC"
        );
    }

    #[test]
    fn test_sql_query_escapes_injection_attempt() {
        let malicious = EscapedLiteral::new("'; DROP TABLE users; --");

        let query = sql_query!("SELECT * FROM users WHERE name = {}", malicious);

        // The quote is doubled, making the malicious input a safe string literal
        assert_eq!(
            query.as_str(),
            "SELECT * FROM users WHERE name = '''; DROP TABLE users; --'"
        );
    }
}
