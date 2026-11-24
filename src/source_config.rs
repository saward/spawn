//! The TOML sidecar next to a migration (`lock.toml`) or a script
//! (`config.toml`), with tests expected to follow.
//!
//! Formats may diverge: shared values live in [`SourceConfig`]; a type with
//! extra fields should define its own struct that `#[serde(flatten)]`s it.

use anyhow::{Context, Result};
use opendal::Operator;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Config values shared by every source type; today just the pin.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct SourceConfig {
    // Reference to the pinned files: an xxhash from spawn's pinner, a git root hash, etc.
    pub pin: String,
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
