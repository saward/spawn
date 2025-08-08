use super::Pinner;
use anyhow::Result;
use async_trait::async_trait;
use object_store::ObjectStore;

/// Uses the latest versions of files, rather than any pinned version.
#[derive(Debug)]
pub struct Latest {}

/// Represents a snapshot of files and folders at a particular point in time.
/// Used to retrieve files as they were at that moment.
impl Latest {
    /// Folder represents the path to our history storage and latest files.
    /// If root is provided then store will use the files from archive rather
    /// than the latest live files.
    pub fn new() -> Result<Self> {
        Ok(Self {})
    }
}

#[async_trait]
impl Pinner for Latest {
    /// Returns the file from the live file system if it exists.
    async fn load(
        &self,
        name: &str,
        object_store: &Box<dyn ObjectStore>,
    ) -> Result<Option<String>> {
        let path_str = format!("components/{}", name);
        let path = object_store::path::Path::from(path_str);

        let get_result = object_store.get(&path).await?;
        let bytes = get_result.bytes().await?;
        let result = Ok::<Vec<u8>, object_store::Error>(bytes.to_vec());
        let res = result.map(|bytes| String::from_utf8(bytes).ok())?;

        Ok(res)
    }

    fn snapshot(&mut self, _object_store: &Box<dyn ObjectStore>) -> Result<String> {
        Err(anyhow::anyhow!("Latest pinner does not support pinning"))
    }
}
