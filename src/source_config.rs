//! The TOML sidecar next to a migration (`lock.toml`) or a script
//! (`config.toml`), with tests expected to follow.
//!
//! Formats may diverge: shared values live in [`SourceConfig`]; a type with
//! extra fields should define its own struct that `#[serde(flatten)]`s it.

use crate::variables::VariableDefinition;
use anyhow::{Context, Result};
use opendal::Operator;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Config values shared by every source type; today just the pin.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct SourceConfig {
    // Reference to the pinned files: an xxhash from spawn's pinner, a git root hash, etc.
    pub pin: String,
}

/// A script's `config.toml`.
///
/// Unlike a migration's `lock.toml`, the pin is optional: a script may have a
/// `config.toml` purely to declare variables, and never be pinned at all.
/// Anything reading this to find a pinned tree must treat `None` as "not
/// pinned" rather than assuming the file's existence implies one.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ScriptConfig {
    /// The pinned component tree's root hash, when the script has been pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin: Option<String>,
    /// Variables the script declares, by name. Ordered so a rewrite by
    /// `spawn script pin` does not reshuffle the file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, VariableDefinition>,
}

/// Reads and parses a source config file at `path`. Callers wanting the failure
/// attributed to a specific migration or script should add their own context.
pub async fn load_source_config<T: DeserializeOwned>(fs: &Operator, path: &str) -> Result<T> {
    let contents = fs
        .read(path)
        .await
        .with_context(|| format!("failed to read source config '{}'", path))?
        .to_bytes();
    let contents = String::from_utf8(contents.to_vec())
        .with_context(|| format!("source config '{}' is not valid UTF-8", path))?;
    toml::from_str(&contents).with_context(|| format!("failed to parse source config '{}'", path))
}
