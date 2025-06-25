use anyhow::Result;
use std::io;

pub mod postgres_psql;

pub trait DatabaseOutputter {
    fn output(&mut self) -> io::Result<Vec<u8>>;
}

pub trait DatabaseWriter: io::Write {
    // outputter consumes self so that no more writing can be done after trying
    // to fetch output.
    fn outputter(self: Box<Self>) -> Result<Box<dyn DatabaseOutputter>>;
}

pub trait Database {
    /// Provides a writer that a given migration can be sent to, so that we can
    /// stream data to this as we go.
    fn new_writer(&self) -> Result<Box<dyn DatabaseWriter>>;
}
