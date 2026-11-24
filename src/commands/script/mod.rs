//! `spawn script` — running a standalone SQL script against the database.
//!
//! Unlike a migration apply, a run is repeatable and resolves includes against
//! the live component tree unless explicitly pinned.

mod build;
mod new;
mod pin;
mod run;

pub use build::BuildScript;
pub use new::NewScript;
pub use pin::{PinScript, PinScriptError};
pub use run::RunScript;

/// Script runs share the migrations' namespace, so scoping migrations also
/// scopes run history.
pub use crate::commands::migration::DEFAULT_NAMESPACE;
