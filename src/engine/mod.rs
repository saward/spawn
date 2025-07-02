use anyhow::Result;
use std::io;

pub mod postgres_psql;

pub struct MigrationStatus {
    applied: bool,
}

pub struct EngineStatus {
    connection_successful: Option<bool>,
}

pub trait EngineOutputter {
    fn output(&mut self) -> io::Result<Vec<u8>>;
}

pub trait EngineWriter: io::Write {
    // outputter consumes self so that no more writing can be done after trying
    // to fetch output.
    fn outputter(self: Box<Self>) -> Result<Box<dyn EngineOutputter>>;
}

pub trait Engine {
    /// Provides a writer that a given migration can be sent to, so that we can
    /// stream data to this as we go.  May not be implemented for all drivers.
    fn new_writer(&self) -> Result<Box<dyn EngineWriter>>;

    fn migration_apply(&self, migration: &str) -> Result<()>;

    // /// Return information about this migration, such as whether it has been
    // /// applied.
    // fn migration_status(&self, checksum: &[u8]) -> anyhow::Result<Status>;

    // /// Performs a check on the connection to see
    // fn check(&self) -> Result<EngineStatus>;
}
