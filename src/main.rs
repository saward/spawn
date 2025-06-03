use spawn::config::{self, Config};
use spawn::migrator::{Migrator, Variables};
use spawn::pinfile::LockData;
use spawn::store;
use sqlx::postgres::PgPoolOptions;
use std::ffi::OsString;
use std::fs;

use anyhow::{Context, Result};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Turn debugging information on
    #[arg(short, long)]
    debug: bool,

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
    /// Run a particular test
    Run { name: OsString },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Load config from file:
    let mut main_config = Config::load().context(format!(
        "could not load config from {}",
        config::MIGRATION_FILE
    ))?;

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
                    let lock_file = main_config.lock_file_path(&migration);
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
                    let pool = PgPoolOptions::new()
                        .max_connections(5)
                        .connect(&main_config.db_connstring)
                        .await?;

                    // Use the sqlx migrator
                    let m =
                        sqlx::migrate::Migrator::new(std::path::Path::new("./migrations")).await?;
                    m.run(&pool).await?;

                    Ok(())
                }
                None => {
                    eprintln!("No migration subcommand specified");
                    Ok(())
                }
            }
        }
        Some(Commands::Test { command }) => {
            match command {
                Some(TestCommands::Run { name }) => {
                    // Blah
                    Ok(())
                }
                None => {
                    eprintln!("No test subcommand specified");
                    Ok(())
                }
            }
        }
        None => Ok(()),
    }
}
