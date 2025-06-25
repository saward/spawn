use anyhow::Result;
use std::io;

pub mod postgres_psql;

pub trait DatabaseOutputter {
    fn output(&mut self) -> io::Result<Vec<u8>>;
}

pub trait DatabaseWriter: io::Write {
    fn outputter(self: Box<Self>) -> Box<dyn DatabaseOutputter>;
}

pub trait Database {
    /// Provides a writer that a given migration can be sent to, so that we can
    /// stream data to this as we go.
    fn new_writer(&self) -> Result<Box<dyn DatabaseWriter>>;
}
