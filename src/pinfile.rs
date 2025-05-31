use serde::{Deserialize, Serialize};

// The overall config, containing a map from filename → FileEntry.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct LockData {
    pub pin: String,
}
