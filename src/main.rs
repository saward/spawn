use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use twox_hash::xxhash3_128;

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
        migration: OsString,
    },
    /// Build a migration into SQL
    Build {
        /// Whether to use pinned components
        #[arg(long)]
        pinned: bool,
        /// Migration to build
        migration: OsString,
    },
}

/// Configuration for template generation
#[derive(Debug)]
struct GenerateConfig {
    /// Base path for all migration related files
    base_path: PathBuf,
    /// Path for the script itself, set to the location under the migrations
    /// folder.
    script_path: OsString,
    /// Variables to pass to the template
    variables: HashMap<String, String>,
    /// Whether to use pinned components
    use_pinned: bool,
}

impl GenerateConfig {
    fn new(
        base_path: PathBuf,
        script_path: OsString,
        variables: Option<HashMap<String, String>>,
        use_pinned: bool,
    ) -> Self {
        GenerateConfig {
            base_path,
            script_path,
            variables: variables.unwrap_or_default(),
            use_pinned,
        }
    }

    // temp_config is to be replaced eventually with a proper way of filling
    // this out.  For now, a single function that returns the config so that we
    // can test, and easily find all places to replace later.
    fn temp_config(migration: &OsString, use_pinned: bool) -> Self {
        GenerateConfig::new(
            PathBuf::from("./static/example"),
            migration.clone(),
            None,
            use_pinned,
        )
    }

    fn pinned_folder(&self) -> PathBuf {
        self.base_path.join("pinned")
    }

    fn components_folder(&self) -> PathBuf {
        self.base_path.join("components")
    }

    fn migrations_folder(&self) -> PathBuf {
        self.base_path.join("migrations")
    }

    fn script_file_path(&self) -> PathBuf {
        self.migrations_folder().join(self.script_path.clone())
    }

    fn lock_file_path(&self) -> PathBuf {
        // Nightly has an add_extension that might be good to use one day if it
        // enters stable.
        let mut lock_file_name = self.script_path.clone();
        lock_file_name.push(".lock");

        self.migrations_folder().join(&lock_file_name)
    }
}

#[cfg(test)]
mod tests {
    use crate::GenerateConfig;
    use std::{ffi::OsString, path::PathBuf};

    fn test_config() -> GenerateConfig {
        GenerateConfig::new(
            PathBuf::from("./base_folder"),
            OsString::from("subfolder/migration_script"),
            None,
            false,
        )
    }

    #[test]
    fn script_file_path() {
        let config = test_config();
        assert_eq!(
            PathBuf::from("./base_folder/migrations/subfolder/migration_script"),
            config.script_file_path(),
        );
    }

    #[test]
    fn lock_file_path() {
        let config = test_config();
        assert_eq!(
            PathBuf::from("./base_folder/migrations/subfolder/migration_script.lock"),
            config.lock_file_path(),
        );
    }
}

/// A custom template loader that loads templates on demand and tracks which ones were loaded
#[derive(Debug)]
struct ComponentLoader {
    components_path: PathBuf,
    loaded_files: Mutex<HashMap<String, String>>,
}

impl ComponentLoader {
    fn new(components_path: PathBuf) -> Self {
        Self {
            components_path,
            loaded_files: Mutex::new(HashMap::new()),
        }
    }

    fn get_loaded_files(&self) -> HashMap<String, String> {
        self.loaded_files.lock().unwrap().clone()
    }

    fn load(&self, name: &str) -> Result<Option<String>, minijinja::Error> {
        let file_path = self.components_path.join(name);
        if let Ok(contents) = std::fs::read_to_string(&file_path) {
            // Track that we loaded this file
            self.loaded_files
                .lock()
                .unwrap()
                .insert(name.to_string(), contents.clone());
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
                let config = GenerateConfig::temp_config(migration, false);
                match generate(&config) {
                    Ok(result) => {
                        for (_name, content) in result.files {
                            let hash = xxhash3_128::Hasher::oneshot(result.content.as_bytes());
                            let hash = format!("{:032x}", hash);
                            let dir = config.pinned_folder().join(&hash[..2]);
                            let file = PathBuf::from(&hash[2..]);

                            let lock_file = config.lock_file_path();

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
                        ()
                    }
                    Err(e) => return Err(e),
                };
                Ok(())
            }
            Some(MigrationCommands::Build { migration, pinned }) => {
                if *pinned {
                    todo!("Pinned migrations not yet implemented")
                }
                let config = GenerateConfig::temp_config(migration, false);
                match generate(&config) {
                    Ok(result) => {
                        println!("{}", result.content);
                        ()
                    }
                    Err(e) => return Err(e),
                };
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

struct Generation {
    content: String,
    files: HashMap<String, String>,
}

/// Opens the specified script file and generates a migration script, compiled
/// using minijinja.
fn generate(config: &GenerateConfig) -> Result<Generation> {
    let mut env = Environment::new();

    // Add our migration script to environment:
    let script_path = config.script_file_path().canonicalize().context(format!(
        "Invalid script path for '{:?}'",
        config.script_file_path()
    ))?;

    let contents = std::fs::read_to_string(&script_path).context(format!(
        "Failed to read migration script '{}'",
        script_path.display()
    ))?;
    env.add_template("migration.sql", &contents)?;

    // Create and set up the component loader
    let components_path = if config.use_pinned {
        config.pinned_folder()
    } else {
        config.components_folder()
    };

    let loader = Arc::new(ComponentLoader::new(components_path));
    let loader_for_closure = loader.clone();
    env.set_loader(move |name| loader_for_closure.load(name));

    // Render with provided variables
    let tmpl = env.get_template("migration.sql")?;
    let content = tmpl.render(context!(variables => config.variables))?;

    // Print which files were loaded (for debugging/verification)
    if cfg!(debug_assertions) {
        eprintln!("Loaded files during template rendering:");
        for (name, _) in loader.get_loaded_files() {
            eprintln!("  - {}", name);
        }
    }

    let result = Generation {
        content: content.to_string(),
        files: loader.get_loaded_files(),
    };

    Ok(result)
}
