use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use walkdir::DirEntry;

use anyhow::{Context, Result};
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

/// A custom template loader that loads templates on demand and tracks which ones were loaded
#[derive(Debug)]
struct ComponentLoader {
    components_path: PathBuf,
    loaded_files: Mutex<HashSet<String>>,
}

impl ComponentLoader {
    fn new(components_path: PathBuf) -> Self {
        Self {
            components_path,
            loaded_files: Mutex::new(HashSet::new()),
        }
    }

    fn get_loaded_files(&self) -> HashSet<String> {
        self.loaded_files.lock().unwrap().clone()
    }

    fn load(&self, name: &str) -> Result<Option<String>, minijinja::Error> {
        let file_path = self.components_path.join(name);
        if let Ok(contents) = std::fs::read_to_string(&file_path) {
            // Track that we loaded this file
            self.loaded_files.lock().unwrap().insert(name.to_string());
            Ok(Some(contents))
        } else {
            Ok(None)
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
        Some(Commands::Migration { command }) => match command {
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
        },
        None => Ok(()),
    }
}

/// Opens the specified script file and generates a migration script, compiled
/// using minijinja.
fn generate(script: &String, config: GenerateConfig) -> Result<()> {
    let mut env = Environment::new();

    // Add our migration script to environment:
    let script_path = config
        .base_path
        .join("migrations")
        .join(script)
        .canonicalize()
        .context(format!("Invalid script path for '{}'", script))?;

    let contents = std::fs::read_to_string(&script_path).context(format!(
        "Failed to read migration script '{}'",
        script_path.display()
    ))?;
    env.add_template("migration.sql", &contents)?;

    // Create and set up the component loader
    let components_path = if config.use_pinned {
        config.base_path.join("pinned")
    } else {
        config.base_path.join("components")
    };

    let loader = Arc::new(ComponentLoader::new(components_path));
    let loader_for_closure = loader.clone();
    env.set_loader(move |name| loader_for_closure.load(name));

    // Render with provided variables
    let tmpl = env.get_template("migration.sql")?;
    println!("{}", tmpl.render(context!(variables => config.variables))?);

    // Print which files were loaded (for debugging/verification)
    if cfg!(debug_assertions) {
        eprintln!("Loaded files during template rendering:");
        for name in loader.get_loaded_files() {
            eprintln!("  - {}", name);
        }
    }

    Ok(())
}
