use serde::{Deserialize, Serialize};

// The overall config, containing a map from filename → FileEntry.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct LockData {
    // Reference to the pinned files.  Might be an xxhash for spawn's pinning
    // system, or a specific git root object hash, etc.
    pub pin: String,
}
