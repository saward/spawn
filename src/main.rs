use std::path::PathBuf;
use walkdir::{DirEntry, WalkDir};
use std::collections::HashMap;

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

/// Configuration for template generation
#[derive(Debug)]
struct GenerateConfig {
    /// Base path for all migration related files
    base_path: PathBuf,
    /// Variables to pass to the template
    variables: HashMap<String, String>,
    /// Whether to use pinned components
    use_pinned: bool,
}

impl Default for GenerateConfig {
    fn default() -> Self {
        Self {
            base_path: PathBuf::from("./static/example"),
            variables: HashMap::new(),
            use_pinned: false,
        }
    }
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
                    let config = GenerateConfig {
                        use_pinned: *pinned,
                        ..Default::default()
                    };
                    generate(migration, config)
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

/// Opens the specified script file and generates a migration script, compiled
/// using minijinja.
fn generate(script: &String, config: GenerateConfig) -> Result<()> {
    let mut env = Environment::new();
    
    // Add our migration script to environment:
    let script_path = config.base_path
        .join("migrations")
        .join(script)
        .canonicalize()
        .context(format!("Invalid script path for '{}'", script))?;
    
    let contents = std::fs::read_to_string(&script_path)
        .context(format!("Failed to read migration script '{}'", script_path.display()))?;
    env.add_template("migration.sql", &contents)?;

    // Load components based on whether we're using pinned or current versions.
    // Currently not implemented correctly for pinned migrations.
    let components_path = if config.use_pinned {
        config.base_path.join("pinned")
    } else {
        config.base_path.join("components")
    };

    // Add components to environment:
    let walker = WalkDir::new(&components_path).into_iter();
    for entry in walker.filter_entry(|e| !is_hidden(e)) {
        let entry = entry?;
        if entry.path().is_file() {
            let entry_path = entry.path();
            let stripped_path = entry_path
                .strip_prefix(&components_path)?
                .to_str()
                .ok_or(anyhow!("Invalid path encoding for component: {}", entry_path.display()))?
                .to_string();
                
            let contents = std::fs::read_to_string(entry_path)
                .context(format!("Failed to read component '{}'", stripped_path))?;
            
            env.add_template_owned(stripped_path, contents)?;
        }
    }

    // Render with provided variables
    let tmpl = env.get_template("migration.sql")?;
    println!("{}", tmpl.render(context!(variables => config.variables))?);

    Ok(())
}
