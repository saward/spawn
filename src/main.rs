use std::path::PathBuf;
use walkdir::{DirEntry, WalkDir};

use anyhow::{anyhow, Context, Result};
use minijinja::{context, Environment};

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
        /// Name of the migration in kebab-case
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
        /// Migration to build
        migration: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.debug {
        false => eprintln!("Debug mode is off"),
        true => eprintln!("Debug mode is on"),
    }

    match &cli.command {
        Some(Commands::Init) => {
            todo!("Implement init command")
        }
        Some(Commands::Migration { command }) => {
            match command {
                Some(MigrationCommands::New { name }) => {
                    todo!("Implement migration new command for {}", name)
                }
                Some(MigrationCommands::Pin { migration }) => {
                    todo!("Implement migration pin command for {}", migration)
                }
                Some(MigrationCommands::Build { migration, pinned }) => {
                    if *pinned {
                        todo!("Pinned migrations not yet implemented")
                    }
                    generate(migration)
                }
                None => {
                    eprintln!("No migration subcommand specified");
                    Ok(())
                }
            }
        }
        None => Ok(())
    }
}

fn is_hidden(entry: &DirEntry) -> bool {
    entry
        .file_name()
        .to_str()
        .map(|s| s.starts_with("."))
        .unwrap_or(false)
}

// Opens the specified script file and generates a migration script, compiled
// using minijinja.
fn generate(script: &String) -> Result<()> {
    let mut env = Environment::new();
    let base_path = format!("./static/example");

    // Add our migration script to environment:
    let script_path = PathBuf::from(format!("{}/templates/{}", &base_path, script));
    let contents = std::fs::read_to_string(script_path).context("could not open script")?;
    env.add_template("migration.sql", &contents)?;

    // Add components to environment:
    let components_path = PathBuf::from(format!("{}/components", &base_path));
    let walker = WalkDir::new(components_path).into_iter();
    for entry in walker.filter_entry(|e| !is_hidden(e)) {
        let entry = entry?;
        if entry.path().is_file() {
            let entry_path = entry.clone().into_path().clone();
            let stripped_path = entry_path
                .strip_prefix(&base_path)?
                .to_str()
                .ok_or(anyhow!("could not strip base path from path"))?
                .to_string();
            let contents =
                std::fs::read_to_string(entry.into_path()).context("could not open script")?;
            env.add_template_owned(stripped_path, contents)?;
        }
    }

    let tmpl = env.get_template("migration.sql")?;
    println!("{}", tmpl.render(context!(name => "John"))?);

    Ok(())
}
