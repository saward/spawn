use migrator::config::{self, Config};
use migrator::migrator::Migrator;
use migrator::pinfile::{LockData, LockEntry};
use sqlx::postgres::PgPoolOptions;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use twox_hash::xxhash3_128;

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
    },
    /// Apply will apply this migration to the database if not already applied,
    /// or all migrations if called without argument.
    Apply { migration: Option<OsString> },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.debug {
        false => eprintln!("Debug mode is off"),
        true => eprintln!("Debug mode is on"),
    }

    match &cli.command {
        Some(Commands::Init) => {
            todo!("Implement init command")
        }
        Some(Commands::Migration { command }) => match command {
            Some(MigrationCommands::New { name }) => {
                let migration_name: String =
                    format!("{}-{}", chrono::Utc::now().format("%Y%m%d%H%M%S"), name);
                println!("creating migration with name {}", &migration_name);
                let mg = Migrator::temp_config(&migration_name.into(), false);

                mg.create_migration()
            }
            Some(MigrationCommands::Pin { migration }) => {
                let config = Migrator::temp_config(migration, false);
                match config.generate() {
                    Ok(result) => {
                        let mut lock_data: LockData = Default::default();
                        for (name, content) in result.files {
                            let hash = xxhash3_128::Hasher::oneshot(result.content.as_bytes());
                            let hash = format!("{:032x}", hash);
                            let dir = config.pinned_folder().join(&hash[..2]);
                            let file = PathBuf::from(&hash[2..]);

                            lock_data.entries.insert(name, LockEntry { hash });

                            fs::create_dir_all(&dir)
                                .context(format!("could not create all dir at {:?}", &dir))?;
                            let path = dir.join(file);

                            if !std::path::Path::new(&path).exists() {
                                let mut f = fs::File::create(&path)
                                    .context(format!("could not create file at {:?}", &path))?;
                                f.write_all(content.as_bytes())
                                    .context("could not write bytes")?;
                            }
                        }
                        let lock_file = config.lock_file_path();
                        let toml_str = toml::to_string_pretty(&lock_data)?;
                        fs::write(lock_file, toml_str)?;
                        ()
                    }
                    Err(e) => return Err(e),
                };
                Ok(())
            }
            Some(MigrationCommands::Build { migration, pinned }) => {
                let config = Migrator::temp_config(migration, *pinned);
                match config.generate() {
                    Ok(result) => {
                        println!("{}", result.content);
                        ()
                    }
                    Err(e) => return Err(e),
                };
                Ok(())
            }
            Some(MigrationCommands::Apply { migration }) => {
                // Load config from file:
                let main_config = Config::load().context(format!(
                    "could not load config from {}",
                    config::MIGRATION_FILE
                ))?;

                let pool = PgPoolOptions::new()
                    .max_connections(5)
                    .connect(&main_config.db_connstring)
                    .await?;

                // Use the sqlx migrator
                let m = sqlx::migrate::Migrator::new(std::path::Path::new("./migrations")).await?;
                m.run(&pool).await?;

                Ok(())
            }
            None => {
                eprintln!("No migration subcommand specified");
                Ok(())
            }
        },
        None => Ok(()),
    }
}
