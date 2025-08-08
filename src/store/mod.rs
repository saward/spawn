use anyhow::Result;
use object_store::ObjectStore;

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
}
