use crate::migrator::Migrator;
use crate::pinfile::LockData;
use crate::sqltest::Tester;
use crate::store::pinner::spawn::Spawn;
use crate::variables::Variables;
use crate::{config::Config, store::pinner::Pinner};
use futures::TryStreamExt;
use opendal::Operator;

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
        migration: String,
    },
    /// Build a migration into SQL
    Build {
        /// Whether to use pinned components
        #[arg(long)]
        pinned: bool,
        /// Migration to build.  Looks for up.sql inside this specified
        /// migration folder.
        migration: String,
        variables: Option<Variables>,
    },
    /// Apply will apply this migration to the database if not already applied,
    /// or all migrations if called without argument.
    Apply {
        /// Whether to use pinned components
        #[arg(long)]
        pinned: bool,

        migration: Option<String>,
        variables: Option<Variables>,
    },
}

#[derive(Subcommand)]
pub enum TestCommands {
    Build {
        name: String,
    },
    /// Run a particular test
    Run {
        name: String,
    },
    /// Run tests and compare to expected.  Runs all tests if no name provided.
    Compare {
        name: Option<String>,
    },
    Expect {
        name: String,
    },
}

pub enum Outcome {
    NewMigration(String),
    BuiltMigration { content: String },
    AppliedMigrations,
    Unimplemented,
    PinnedMigration { hash: String },
}

pub async fn run_cli(cli: Cli, base_op: &Operator) -> Result<Outcome> {
    // Load config from file:
    let mut main_config = Config::load(&cli.config_file, &base_op, cli.database)
        .await
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
                    let mg = Migrator::new(&main_config, &migration_name, false);

                    Ok(Outcome::NewMigration(mg.create_migration().await?))
                }
                Some(MigrationCommands::Pin { migration }) => {
                    let mut pinner =
                        Spawn::new(main_config.pinned_folder(), main_config.components_folder())
                            .context("could not get pinned_folder")?;

                    let root = pinner
                        .snapshot(&main_config.operator())
                        .await
                        .context("error calling pinner snapshot")?;
                    let lock_file_path = main_config.migration_lock_file_path(&migration);
                    let toml_str = toml::to_string_pretty(&LockData { pin: root.clone() })
                        .context("could not not convert pin data to toml")?;
                    base_op
                        .write(&lock_file_path, toml_str)
                        .await
                        .context("failed writing migration lockfile")?;

                    Ok(Outcome::PinnedMigration { hash: root })
                }
                Some(MigrationCommands::Build {
                    migration,
                    pinned,
                    variables,
                }) => {
                    let mgrtr = Migrator::new(&main_config, &migration, *pinned);
                    match mgrtr.generate(variables.clone()).await {
                        Ok(result) => Ok(Outcome::BuiltMigration {
                            content: result.content,
                        }),
                        Err(e) => return Err(e),
                    }
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
                        let mgrtr = Migrator::new(&main_config, &migration, *pinned);
                        match mgrtr.generate(variables.clone()).await {
                            Ok(result) => {
                                let engine = main_config.new_engine()?;
                                engine
                                    .migration_apply(&result.content)
                                    .await
                                    .context(format!(
                                        "Failed to apply migration '{}'",
                                        &migration,
                                    ))?;
                                println!("Migration '{}' applied successfully", &migration);
                                ()
                            }
                            Err(e) => {
                                return Err(e.context(anyhow::anyhow!(format!(
                                    "failed to generate migration '{}'",
                                    &migration,
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
                let config = Tester::new(&main_config, &name);
                match config.generate(None).await {
                    Ok(result) => {
                        println!("{}", result);
                        ()
                    }
                    Err(e) => return Err(e),
                };
                Ok(Outcome::Unimplemented)
            }
            Some(TestCommands::Run { name }) => {
                let config = Tester::new(&main_config, &name);
                match config.run(None).await {
                    Ok(result) => {
                        println!("{}", result);
                        ()
                    }
                    Err(e) => return Err(e),
                };
                Ok(Outcome::Unimplemented)
            }
            Some(TestCommands::Compare { name }) => {
                let test_files: Vec<String> = match name {
                    Some(name) => vec![name.clone()],
                    None => {
                        let mut tests: Vec<String> = Vec::new();
                        let mut fs_lister = main_config
                            .operator()
                            .lister(&main_config.tests_folder())
                            .await?;
                        while let Some(entry) = fs_lister.try_next().await? {
                            let path = entry.path().to_string();
                            if path.ends_with("/") {
                                tests.push(path)
                            }
                        }

                        tests
                    }
                };

                let mut failed = false;

                for test_file in test_files {
                    let config = Tester::new(&main_config, &test_file);

                    match config.run_compare(None).await {
                        Ok(result) => match result.diff {
                            None => {
                                println!("{}[PASS]{} {}", GREEN, RESET, test_file);
                            }
                            Some(diff) => {
                                failed = true;
                                println!("\n{}[FAIL]{} {}{}{}", RED, RESET, BOLD, test_file, RESET);
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
                let tester = Tester::new(&main_config, &name);
                match tester.save_expected(None).await {
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
