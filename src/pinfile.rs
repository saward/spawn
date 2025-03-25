use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// A single file entry with its hash.
#[derive(Debug, Deserialize, Serialize)]
pub struct LockEntry {
    pub hash: String,
}

// The overall config, containing a map from filename → FileEntry.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct LockData {
    pub entries: HashMap<String, LockEntry>,
}
