use std::sync::Arc;

use anyhow::Result;
use object_store::{local::LocalFileSystem, memory::InMemory, ObjectStore};

use crate::store::pinner::Pinner;

pub mod fs;
pub mod pinner;

pub trait FS {}

pub struct Store {
    pinner: Box<dyn Pinner>,
    fs: Box<dyn ObjectStore>,
}

impl Store {
    pub fn new(pinner: Box<dyn Pinner>, fs: Box<dyn ObjectStore>) -> Result<Store> {
        Ok(Store { pinner, fs })
    }

    pub fn load(&self, name: &str) -> std::result::Result<Option<String>, minijinja::Error> {
        self.pinner.load(name, &self.fs)
    }
}
