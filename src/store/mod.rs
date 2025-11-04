use anyhow::Result;
use object_store::{path::Path, ObjectStore};

use crate::store::pinner::Pinner;

pub mod pinner;

pub struct Store {
    pinner: Box<dyn Pinner>,
    fs: Box<dyn ObjectStore>,
}

impl Store {
    pub fn new(pinner: Box<dyn Pinner>, fs: Box<dyn ObjectStore>) -> Result<Store> {
        Ok(Store { pinner, fs })
    }

    pub async fn load_component(&self, name: &str) -> Result<Option<String>> {
        let res = self.pinner.load(name, &self.fs).await?;

        Ok(res)
    }

    pub async fn load_migration(&self, name: &Path) -> Result<String> {
        // Append the migration folder name to the path:
        let name = Path::from(format!("migrations/{}", name.to_string()));
        let result = self.fs.get(&name).await?;
        let bytes = result.bytes().await?;
        let contents = String::from_utf8(bytes.to_vec())?;

        Ok(contents)
    }
}
