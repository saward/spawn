use super::Pinner;
use anyhow::Result;
use async_trait::async_trait;
use opendal::Operator;

/// Uses the latest versions of files, rather than any pinned version.
#[derive(Debug)]
pub struct Latest {
    store_path: String,
}

/// Represents a snapshot of files and folders at a particular point in time.
/// Used to retrieve files as they were at that moment.
impl Latest {
    /// Folder represents the path to our history storage and latest files.
    /// If root is provided then store will use the files from archive rather
    /// than the latest live files.
    pub fn new(store_path: &str) -> Result<Self> {
        Ok(Self {
            store_path: store_path.to_string(),
        })
    }
}

#[async_trait]
impl Pinner for Latest {
    /// Returns the file from the live file system if it exists.
    async fn load_bytes(&self, name: &str, object_store: &Operator) -> Result<Option<Vec<u8>>> {
        let path_str = format!("{}/components/{}", self.store_path, name);

        let get_result = object_store.read(&path_str).await?;
        let bytes = get_result.to_bytes();

        Ok(Some(bytes.to_vec()))
    }

    async fn snapshot(&mut self, _object_store: &Operator) -> Result<String> {
        Err(anyhow::anyhow!("Latest pinner does not support pinning"))
    }
}
