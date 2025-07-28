use std::sync::Arc;

use anyhow::Result;

use crate::store::pinner::Pinner;

pub mod fs;
pub mod pinner;

pub trait FS {}

pub struct Store {
    pinner: Box<dyn Pinner>,
}

impl Store {
    pub fn new(pinner: Box<dyn Pinner>) -> Result<Store> {
        Ok(Store { pinner: pinner })
    }

    pub fn load(&self, name: &str) -> std::result::Result<Option<String>, minijinja::Error> {
        self.pinner.load(name)
    }
}
