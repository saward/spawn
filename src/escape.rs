//! Type-safe SQL escaping for PostgreSQL.
//!
//! This module provides wrapper types that guarantee SQL values have been properly
//! escaped at construction time. By using these types instead of raw strings,
//! the type system ensures that escaping cannot be forgotten.
//!
//! # Example
//!
//! ```
//! use spawn::escape::{EscapedIdentifier, EscapedLiteral};
//!
//! let schema = EscapedIdentifier::new("my_schema");
//! let value = EscapedLiteral::new("user's input");
//!
//! let sql = format!(
//!     "SELECT * FROM {}.users WHERE name = {}",
//!     schema.as_str(),
//!     value.as_str()
//! );
//! ```

use postgres_protocol::escape::{escape_identifier, escape_literal};
use std::fmt;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escaped_identifier_basic() {
        // Verify our wrapper correctly calls through to postgres_protocol
        let ident = EscapedIdentifier::new("my_schema");
        assert_eq!(ident.as_str(), "\"my_schema\"");
    }

    #[test]
    fn test_escaped_literal_basic() {
        // Verify our wrapper correctly calls through to postgres_protocol
        let lit = EscapedLiteral::new("hello");
        assert_eq!(lit.as_str(), "'hello'");
    }
}
