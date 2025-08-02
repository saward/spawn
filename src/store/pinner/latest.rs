use super::Pinner;
use anyhow::Result;
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

impl Pinner for Latest {
    /// Returns the file from the live file system if it exists.
    fn load(
        &self,
        name: &str,
        object_store: &Box<dyn ObjectStore>,
    ) -> std::result::Result<Option<String>, minijinja::Error> {
        let path_str = format!("components/{}", name);
        let path = object_store::path::Path::from(path_str);
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let get_result = object_store.get(&path).await?;
                let bytes = get_result.bytes().await?;
                Ok::<Vec<u8>, object_store::Error>(bytes.to_vec())
            })
        });
        result
            .map(|bytes| String::from_utf8(bytes).ok())
            .map_err(|e| {
                minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    "Failed to load from object store",
                )
                .with_source(e)
            })
    }

    fn snapshot(&mut self) -> Result<String> {
        Err(anyhow::anyhow!("Latest pinner does not support pinning"))
    }
}
