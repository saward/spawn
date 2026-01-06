use anyhow::{Context, Result};
use futures::TryStreamExt;
use opendal::services::Memory;
use opendal::Operator;
use std::fmt::Debug;

use crate::store::pinner::Pinner;

pub mod pinner;

pub struct Store {
    pinner: Box<dyn Pinner>,
    fs: Operator,
    spawn_folder: String,
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
    pub fn new(pinner: Box<dyn Pinner>, fs: Operator, spawn_folder: &str) -> Result<Store> {
        // We need subdirectory passed in, because we can't guarantee that
        // operator is set to the root spawn folder.  The reason why operator
        // isn't guaranteed is because doing so made it tricky to implement
        // some tests, for reasons I cannot recall, but relate to the
        // possibility that the location of the config file may or may not
        // be the same filesystem as where the other spawn files are.  If
        // opendal one day supports the ability to get a new operator from
        // an old, with a new root set, that will solve issue.  RFC:
        // https://github.com/apache/opendal/blob/main/core/core/src/docs/rfcs/3197_config.md
        Ok(Store {
            pinner,
            fs,
            spawn_folder: spawn_folder.to_string(),
        })
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

pub enum DesiredOperator {
    Memory,
    FileSystem,
}

// Handy function for getting a disk based folder of data and reeturn an in
// memory operator that has the same contents.  Particularly useful for tests.
pub async fn disk_to_operator(
    source_folder: &str,
    dest_prefix: Option<&str>,
    desired_operator: DesiredOperator,
) -> Result<Operator> {
    let dest_op = match desired_operator {
        DesiredOperator::FileSystem => {
            let dest_service = opendal::services::Fs::default().root("./testout");
            Operator::new(dest_service)?.finish()
        }
        DesiredOperator::Memory => {
            let dest_service = Memory::default();
            Operator::new(dest_service)?.finish()
        }
    };

    // Create a LocalFileSystem to read from static/example
    let fs_service = opendal::services::Fs::default().root(source_folder);
    let source_store = Operator::new(fs_service)
        .context("disk_to_mem_operator failed to create operator")?
        .finish();

    // Populate the in-memory store with contents from static/example
    let store_loc = dest_prefix.unwrap_or_default();
    crate::store::populate_store_from_store(&source_store, &dest_op, "", store_loc)
        .await
        .context("call to populate memory fs from object store")?;

    Ok(dest_op)
}

pub async fn populate_store_from_store(
    source_store: &Operator,
    target_store: &Operator,
    source_prefix: &str,
    dest_prefix: &str,
) -> Result<()> {
    let mut lister = source_store
        .lister_with(source_prefix)
        .recursive(true)
        .await
        .context("lister call")?;
    let mut list_result: Vec<opendal::Entry> = Vec::new();

    println!("Trying to write all");
    while let Some(entry) = lister.try_next().await? {
        println!("found {}", entry.path());
        if entry.path().ends_with("/") {
            continue;
        }
        list_result.push(entry);
    }

    for entry in list_result {
        // Print out the file we're writing:
        let dest_object_path = format!("{}{}", dest_prefix, entry.path());
        let source_object_path = entry.path();
        println!("Writing {} to {}", &source_object_path, &dest_object_path);

        // Get the object data
        let bytes = source_store
            .read(&source_object_path)
            .await
            .context(format!("read path {}", &source_object_path))?;

        // Store in target with the same path
        target_store
            .write(&dest_object_path, bytes)
            .await
            .context("write")?;
    }

    Ok(())
}
