// This is a driver that uses a locally provided PSQL command to execute
// scripts, which enables user's scripts to take advantage of things like the
// build in PSQL helper commands.

use crate::dbdriver::{Database, DatabaseOutputter, DatabaseWriter};
use anyhow::Result;
use std::io::{self, Read};
use std::process::{Child, ChildStdin, Command, Stdio};

#[derive(Debug)]
pub struct PSQL {
    psql_command: Vec<String>,
}

pub struct PSQLWriter {
    child: Child,
    stdin: ChildStdin,
}

pub struct PSQLOutput {
    child: Child,
}

impl PSQL {
    pub fn new(psql_command: &Vec<String>) -> Box<dyn Database> {
        Box::new(Self {
            psql_command: psql_command.clone(),
        })
    }
}

impl crate::dbdriver::Database for PSQL {
    fn new_writer(&self) -> Result<Box<dyn DatabaseWriter>> {
        let mut parts = self.psql_command.clone();
        let command = parts.remove(0);
        let mut child = &mut Command::new(command);
        for arg in parts {
            child = child.arg(arg);
        }
        let mut child = child
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to execute command");

        let stdin = child
            .stdin
            .take()
            .ok_or(anyhow::anyhow!("no stdin found"))?;

        Ok(Box::new(PSQLWriter { child, stdin }))
    }
}

impl crate::dbdriver::DatabaseOutputter for PSQLOutput {
    fn output(&mut self) -> io::Result<Vec<u8>> {
        // Collect all output
        let mut output = Vec::new();
        if let Some(mut out) = self.child.stdout.take() {
            out.read_to_end(&mut output)?;
        }

        // Reap the child to avoid a zombie process
        let _ = self.child.wait()?;
        Ok(output)
    }
}

impl crate::dbdriver::DatabaseWriter for PSQLWriter {
    fn outputter(self: Box<Self>) -> Box<dyn DatabaseOutputter> {
        Box::new(PSQLOutput { child: self.child })
    }
}

impl io::Write for PSQLWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stdin.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stdin.flush()
    }
}
