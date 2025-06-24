use anyhow::Result;
use std::io;

pub mod postgres_psql;

pub trait DatabaseWriter: io::Write {
    fn finish(&self);
}

pub trait Database<T: DatabaseWriter> {
    /// Provides a writer that a given migration can be sent to, so that we can
    /// stream data to this as we go.
    fn get_dbwriter(&self) -> Result<T>;
}
