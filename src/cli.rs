use crate::migrator::Migrator;
use crate::pinfile::LockData;
use crate::sqltest::Tester;
use crate::store::pinner::spawn::Spawn;
use crate::variables::Variables;
use crate::{config::Config, store::pinner::Pinner};
use object_store::local::LocalFileSystem;
use object_store::ObjectStore;
use std::ffi::OsString;
use std::fs;

use anyhow::{Context, Result};
use chrono;
use clap::{Parser, Subcommand};
use toml;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";

#[derive(Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    /// Turn debugging information on
    #[arg(short, long)]
    pub debug: bool,

    #[arg(global = true, short, long, default_value = "spawn.toml")]
    pub config_file: String,

    #[arg(global = true, long)]
    pub database: Option<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new migration environment
    Init,
    Migration {
        #[command(subcommand)]
        command: Option<MigrationCommands>,
        #[arg(short, long, global = true)]
        environment: Option<String>,
    },
    Test {
        #[command(subcommand)]
        command: Option<TestCommands>,
    },
}

#[derive(Subcommand)]
pub enum MigrationCommands {
    /// Create a new migration with the provided name
    New {
        /// Name of the migration.
        name: String,
    },
    /// Pin a migration with current components
    Pin {
        /// Migration to pin
        migration: OsString,
    },
    /// Build a migration into SQL
    Build {
        /// Whether to use pinned components
        #[arg(long)]
        pinned: bool,
        /// Migration to build.  Looks for script.sql inside this specified
        /// migration folder.
        migration: OsString,
        variables: Option<Variables>,
    },
    /// Apply will apply this migration to the database if not already applied,
    /// or all migrations if called without argument.
    Apply {
        /// Whether to use pinned components
        #[arg(long)]
        pinned: bool,

        migration: Option<OsString>,
        variables: Option<Variables>,
    },
}

#[derive(Subcommand)]
pub enum TestCommands {
    Build {
        name: OsString,
    },
    /// Run a particular test
    Run {
        name: OsString,
    },
    /// Run tests and compare to expected.  Runs all tests if no name provided.
    Compare {
        name: Option<OsString>,
    },
    Expect {
        name: OsString,
    },
}

pub enum Outcome {
    NewMigration(String),
    AppliedMigrations,
    Unimplemented,
}

pub async fn run_cli(cli: Cli) -> Result<Outcome> {
    // Load config from file:
    let mut main_config = Config::load(&cli.config_file, cli.database)
        .context(format!("could not load config from {}", &cli.config_file,))?;

    match &cli.command {
        Some(Commands::Init) => {
            todo!("Implement init command")
        }
        Some(Commands::Migration {
            command,
            environment,
        }) => {
            main_config.environment = environment.clone();
            match command {
                Some(MigrationCommands::New { name }) => {
                    let migration_name: String =
                        format!("{}-{}", chrono::Utc::now().format("%Y%m%d%H%M%S"), name);
                    println!("creating migration with name {}", &migration_name);
                    let mg = Migrator::new(&main_config, migration_name.into(), false);

                    Ok(Outcome::NewMigration(mg.create_migration()?))
                }
                Some(MigrationCommands::Pin { migration }) => {
                    let mut pinner = Spawn::new(
                        main_config.pinned_folder(),
                        main_config.components_folder(),
                        None,
                    )?;

                    let fs: Box<dyn ObjectStore> =
                        Box::new(LocalFileSystem::new_with_prefix(&main_config.spawn_folder)?);

                    let root = pinner.snapshot(&fs)?;
                    let lock_file = main_config.migration_lock_file_path(&migration);
                    let toml_str = toml::to_string_pretty(&LockData { pin: root })?;
                    fs::write(lock_file, toml_str)?;

                    Ok(Outcome::Unimplemented)
                }
                Some(MigrationCommands::Build {
                    migration,
                    pinned,
                    variables,
                }) => {
                    let mgrtr = Migrator::new(&main_config, migration.clone(), *pinned);
                    match mgrtr.generate(variables.clone()) {
                        Ok(result) => {
                            println!("{}", result.content);
                            ()
                        }
                        Err(e) => return Err(e),
                    };
                    Ok(Outcome::Unimplemented)
                }
                Some(MigrationCommands::Apply {
                    migration,
                    pinned,
                    variables,
                }) => {
                    let mut migrations = Vec::new();
                    match migration {
                        Some(migration) => migrations.push(migration.clone()),
                        None => {
                            return Err(anyhow::anyhow!("applying all migrations not implemented"))
                        }
                    }

                    for migration in migrations {
                        let migration_str = migration.to_str().unwrap_or_default();
                        let mgrtr = Migrator::new(&main_config, migration.clone(), *pinned);
                        match mgrtr.generate(variables.clone()) {
                            Ok(result) => {
                                let engine = main_config.new_engine()?;
                                engine.migration_apply(&result.content).context(format!(
                                    "Failed to apply migration '{}'",
                                    &migration_str,
                                ))?;
                                println!("Migration '{}' applied successfully", &migration_str);
                                ()
                            }
                            Err(e) => {
                                return Err(e.context(anyhow::anyhow!(format!(
                                    "failed to generate migration '{}'",
                                    &migration_str
                                ))))
                            }
                        };
                    }
                    Ok(Outcome::AppliedMigrations)
                }

                None => {
                    eprintln!("No migration subcommand specified");
                    Ok(Outcome::Unimplemented)
                }
            }
        }
        Some(Commands::Test { command }) => match command {
            Some(TestCommands::Build { name }) => {
                let config = Tester::new(&main_config, name.clone());
                match config.generate(None) {
                    Ok(result) => {
                        println!("{}", result);
                        ()
                    }
                    Err(e) => return Err(e),
                };
                Ok(Outcome::Unimplemented)
            }
            Some(TestCommands::Run { name }) => {
                let config = Tester::new(&main_config, name.clone());
                match config.run(None) {
                    Ok(result) => {
                        println!("{}", result);
                        ()
                    }
                    Err(e) => return Err(e),
                };
                Ok(Outcome::Unimplemented)
            }
            Some(TestCommands::Compare { name }) => {
                let test_files: Vec<OsString> = match name {
                    Some(name) => vec![name.clone()],
                    None => {
                        let mut tests = Vec::new();
                        // Grab all test files in the folder:
                        for entry in fs::read_dir(main_config.tests_folder())? {
                            let entry = entry?;
                            let path = entry.path();
                            if path.is_dir() {
                                tests.push(
                                    path.file_name()
                                        .ok_or(anyhow::anyhow!("no test found!"))?
                                        .to_os_string(),
                                );
                            }
                        }
                        tests
                    }
                };

                let mut failed = false;

                for test_file in test_files {
                    let config = Tester::new(&main_config, test_file.clone());
                    let name = test_file
                        .into_string()
                        .unwrap_or_else(|_| "<invalid utf8>".to_string());

                    match config.run_compare(None) {
                        Ok(result) => match result.diff {
                            None => {
                                println!("{}[PASS]{} {}", GREEN, RESET, name);
                            }
                            Some(diff) => {
                                failed = true;
                                println!("\n{}[FAIL]{} {}{}{}", RED, RESET, BOLD, name, RESET);
                                println!("{}--- Diff ---{}", BOLD, RESET);
                                println!("{}", diff);
                                println!("{}-------------{}\n", BOLD, RESET);
                            }
                        },
                        Err(e) => return Err(e),
                    };
                }

                if failed {
                    return Err(anyhow::anyhow!(
                        "{}!{} Differences found in one or more tests",
                        RED,
                        RESET
                    ));
                }

                Ok(Outcome::Unimplemented)
            }
            Some(TestCommands::Expect { name }) => {
                let tester = Tester::new(&main_config, name.clone());
                match tester.save_expected(None) {
                    Ok(_) => (),
                    Err(e) => return Err(e),
                };
                Ok(Outcome::Unimplemented)
            }
            None => {
                eprintln!("No test subcommand specified");
                Ok(Outcome::Unimplemented)
            }
        },
        None => Ok(Outcome::Unimplemented),
    }
}
