use spawn::config::Config;
use spawn::migrator::Migrator;
use spawn::pinfile::LockData;
use spawn::sqltest::Tester;
use spawn::store;
use spawn::variables::Variables;
use std::ffi::OsString;
use std::fs;

use anyhow::{Context, Result};

use clap::{Parser, Subcommand};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Turn debugging information on
    #[arg(short, long)]
    debug: bool,

    #[arg(global = true, short, long, default_value = "spawn.toml")]
    config_file: String,

    #[arg(global = true, long)]
    database: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
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
enum MigrationCommands {
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
        migration: Option<OsString>,
        variables: Option<Variables>,
    },
}

#[derive(Subcommand)]
enum TestCommands {
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

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
            main_config.environment = environment.clone().unwrap_or(main_config.environment);
            match command {
                Some(MigrationCommands::New { name }) => {
                    let migration_name: String =
                        format!("{}-{}", chrono::Utc::now().format("%Y%m%d%H%M%S"), name);
                    println!("creating migration with name {}", &migration_name);
                    let mg = Migrator::new(&main_config, migration_name.into(), false);

                    mg.create_migration()
                }
                Some(MigrationCommands::Pin { migration }) => {
                    let root = store::snapshot(
                        &main_config.pinned_folder(),
                        &main_config.components_folder(),
                    )?;
                    let lock_file = main_config.migration_lock_file_path(&migration);
                    let toml_str = toml::to_string_pretty(&LockData { pin: root })?;
                    fs::write(lock_file, toml_str)?;

                    Ok(())
                }
                Some(MigrationCommands::Build {
                    migration,
                    pinned,
                    variables,
                }) => {
                    let config = Migrator::new(&main_config, migration.clone(), *pinned);
                    match config.generate(variables.clone()) {
                        Ok(result) => {
                            println!("{}", result.content);
                            ()
                        }
                        Err(e) => return Err(e),
                    };
                    Ok(())
                }
                Some(MigrationCommands::Apply {
                    migration: _,
                    variables: _,
                }) => {
                    // let pool = PgPoolOptions::new()
                    //     .max_connections(5)
                    //     .connect(&main_config.db_connstring)
                    //     .await?;

                    // // Use the sqlx migrator
                    // let m =
                    //     sqlx::migrate::Migrator::new(std::path::Path::new("./migrations")).await?;
                    // m.run(&pool).await?;

                    Ok(())
                }
                None => {
                    eprintln!("No migration subcommand specified");
                    Ok(())
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
                Ok(())
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
                Ok(())
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

                Ok(())
            }
            Some(TestCommands::Expect { name }) => {
                let tester = Tester::new(&main_config, name.clone());
                match tester.save_expected(None) {
                    Ok(_) => (),
                    Err(e) => return Err(e),
                };
                Ok(())
            }
            None => {
                eprintln!("No test subcommand specified");
                Ok(())
            }
        },
        None => Ok(()),
    }
}
