use anyhow::{Context, Result};
use opendal::Operator;
use serde::{Deserialize, Serialize};

// The overall config, containing a map from filename → FileEntry.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct LockData {
    // Reference to the pinned files.  Might be an xxhash for spawn's pinning
    // system, or a specific git root object hash, etc.
    pub pin: String,
}

/// Reads and parses a lock.toml at `path`. The canonical way to load a
/// migration's lock file — callers that need to attribute a failure to a
/// specific migration should wrap this with their own `.with_context(...)`.
pub async fn load_lock_file(fs: &Operator, path: &str) -> Result<LockData> {
    let contents = fs
        .read(path)
        .await
        .with_context(|| format!("failed to read lock file '{}'", path))?
        .to_bytes();
    let contents = String::from_utf8(contents.to_vec())
        .with_context(|| format!("lock file '{}' is not valid UTF-8", path))?;
    toml::from_str(&contents).with_context(|| format!("failed to parse lock file '{}'", path))
}
