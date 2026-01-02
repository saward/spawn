use anyhow::Result;
use opendal::Operator;
use std::fmt::Debug;

use crate::store::pinner::Pinner;

pub mod pinner;

pub struct Store {
    pinner: Box<dyn Pinner>,
    fs: Operator,
}

impl Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("pinner", &self.pinner)
            .field("fs", &self.fs)
            .finish()
    }
}

impl Store {
    pub fn new(pinner: Box<dyn Pinner>, fs: Operator) -> Result<Store> {
        Ok(Store { pinner, fs })
    }

    pub async fn load_component(&self, name: &str) -> Result<Option<String>> {
        let res = self.pinner.load(name, &self.fs).await?;

        Ok(res)
    }

    pub async fn load_migration(&self, name: &str) -> Result<String> {
        let result = self.fs.read(&name).await?;
        let bytes = result.to_bytes();
        let contents = String::from_utf8(bytes.to_vec())?;

        Ok(contents)
    }
}
