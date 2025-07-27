use anyhow::Result;

use crate::store::pinner::Pinner;

pub mod fs;
pub mod pinner;

pub trait FS {}

#[derive(Clone)]
pub struct Store<P: Pinner> {
    pinner: P,
}

impl<P: Pinner> Store<P> {
    pub fn new(pinner: P) -> Result<Store<P>> {
        Ok(Store { pinner: pinner })
    }

    pub fn load(&self, name: &str) -> std::result::Result<Option<String>, minijinja::Error> {
        self.pinner.load(name)
    }
}
