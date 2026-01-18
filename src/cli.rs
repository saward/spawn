use crate::config::{Config, ConfigLoaderSaver};
use crate::engine::MigrationError;
use crate::migrator::Migrator;
use crate::pinfile::LockData;
use crate::sqltest::Tester;
use crate::store::pinner::spawn::Spawn;
use crate::store::pinner::Pinner;
use futures::TryStreamExt;
use opendal::Operator;

use anyhow::{anyhow, Context, Result};
use chrono;
use clap::{Parser, Subcommand};
use toml;
use uuid::Uuid;

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const RED: &str = "\x1b[31m";
const GREEN: &str = "\x1b[32m";

/// Trait for describing commands in a telemetry-safe way.
///
/// Implementations should return sanitized command strings that don't
/// contain sensitive information like file paths or migration names.
pub trait TelemetryDescribe {
    /// Returns a sanitized command string for telemetry.
    /// Should not contain sensitive values like file paths or migration names.
    fn telemetry_command(&self) -> String;

    /// Returns additional safe properties to include in telemetry.
    /// Only include non-sensitive boolean flags or enum values.
    fn telemetry_properties(&self) -> Vec<(&'static str, String)> {
        vec![]
    }
}

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

impl TelemetryDescribe for Cli {
    fn telemetry_command(&self) -> String {
        match &self.command {
            Some(cmd) => cmd.telemetry_command(),
            None => String::new(),
        }
    }

    fn telemetry_properties(&self) -> Vec<(&'static str, String)> {
        match &self.command {
            Some(cmd) => cmd.telemetry_properties(),
            None => vec![],
        }
    }
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

impl TelemetryDescribe for Commands {
    fn telemetry_command(&self) -> String {
        match self {
            Commands::Init => "init".to_string(),
            Commands::Migration { command, .. } => match command {
                Some(cmd) => format!("migration {}", cmd.telemetry_command()),
                None => "migration".to_string(),
            },
            Commands::Test { command } => match command {
                Some(cmd) => format!("test {}", cmd.telemetry_command()),
                None => "test".to_string(),
            },
        }
    }

    fn telemetry_properties(&self) -> Vec<(&'static str, String)> {
        match self {
            Commands::Init => vec![],
            Commands::Migration { command, .. } => command
                .as_ref()
                .map(|c| c.telemetry_properties())
                .unwrap_or_default(),
            Commands::Test { command } => command
                .as_ref()
                .map(|c| c.telemetry_properties())
                .unwrap_or_default(),
        }
    }
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
        /// Path to a variables file (JSON, TOML, or YAML) to use for templating.
        /// Overrides the variables_file setting in spawn.toml.
        #[arg(long)]
        variables: Option<String>,
    },
    /// Apply will apply this migration to the database if not already applied,
    /// or all migrations if called without argument.
    Apply {
        /// Whether to use pinned components
        #[arg(long)]
        pinned: bool,

        migration: Option<String>,

        /// Path to a variables file (JSON, TOML, or YAML) to use for templating.
        /// Overrides the variables_file setting in spawn.toml.
        #[arg(long)]
        variables: Option<String>,
    },
}

impl TelemetryDescribe for MigrationCommands {
    fn telemetry_command(&self) -> String {
        match self {
            MigrationCommands::New { .. } => "new".to_string(),
            MigrationCommands::Pin { .. } => "pin".to_string(),
            MigrationCommands::Build { .. } => "build".to_string(),
            MigrationCommands::Apply { .. } => "apply".to_string(),
        }
    }

    fn telemetry_properties(&self) -> Vec<(&'static str, String)> {
        match self {
            MigrationCommands::New { .. } => vec![],
            MigrationCommands::Pin { .. } => vec![],
            MigrationCommands::Build {
                pinned, variables, ..
            } => {
                vec![
                    ("opt_pinned", pinned.to_string()),
                    ("has_variables", variables.is_some().to_string()),
                ]
            }
            MigrationCommands::Apply {
                pinned,
                variables,
                migration,
            } => {
                vec![
                    ("opt_pinned", pinned.to_string()),
                    ("has_variables", variables.is_some().to_string()),
                    ("apply_all", migration.is_none().to_string()),
                ]
            }
        }
    }
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

impl TelemetryDescribe for TestCommands {
    fn telemetry_command(&self) -> String {
        match self {
            TestCommands::Build { .. } => "build".to_string(),
            TestCommands::Run { .. } => "run".to_string(),
            TestCommands::Compare { .. } => "compare".to_string(),
            TestCommands::Expect { .. } => "expect".to_string(),
        }
    }

    fn telemetry_properties(&self) -> Vec<(&'static str, String)> {
        match self {
            TestCommands::Compare { name } => {
                vec![("compare_all", name.is_none().to_string())]
            }
            _ => vec![],
        }
    }
}

pub enum Outcome {
    NewMigration(String),
    BuiltMigration { content: String },
    AppliedMigrations,
    Unimplemented,
    PinnedMigration { hash: String },
    Success,
}

/// Result of running the CLI, including telemetry information
pub struct CliResult {
    pub outcome: Result<Outcome>,
    /// Project ID from config (for telemetry distinct_id)
    pub project_id: Option<String>,
    /// Whether telemetry is enabled in config
    pub telemetry_enabled: bool,
}

pub async fn run_cli(cli: Cli, base_op: &Operator) -> CliResult {
    // Handle init command separately as it doesn't require existing config
    if let Some(Commands::Init) = &cli.command {
        return run_init(&cli, base_op).await;
    }

    // Load config from file (required for all other commands)
    let main_config = match Config::load(&cli.config_file, base_op, cli.database.clone()).await {
        Ok(cfg) => cfg,
        Err(e) => {
            return CliResult {
                outcome: Err(e.context(format!("could not load config from {}", &cli.config_file))),
                project_id: None,
                telemetry_enabled: true, // Default to enabled if we can't load config
            };
        }
    };

    // Extract telemetry info from config
    let project_id = main_config.project_id.clone();
    let telemetry_enabled = main_config.telemetry;

    // Run the actual command
    let outcome = run_command(cli, main_config, base_op).await;

    CliResult {
        outcome,
        project_id,
        telemetry_enabled,
    }
}

async fn run_init(cli: &Cli, base_op: &Operator) -> CliResult {
    // Check if spawn.toml already exists
    let config_exists = match base_op.exists(&cli.config_file).await {
        Ok(exists) => exists,
        Err(e) => {
            return CliResult {
                outcome: Err(e.into()),
                project_id: None,
                telemetry_enabled: true,
            };
        }
    };

    if config_exists {
        return CliResult {
            outcome: Err(anyhow!(
                "Config file '{}' already exists. Use a different path or remove the existing file.",
                &cli.config_file
            )),
            project_id: None,
            telemetry_enabled: true,
        };
    }

    // Generate a new project_id
    let project_id = Uuid::new_v4().to_string();

    // Create default config
    let config = ConfigLoaderSaver {
        spawn_folder: "spawn".to_string(),
        database: None,
        environment: None,
        databases: None,
        project_id: Some(project_id.clone()),
        telemetry: true,
    };

    // Save the config
    if let Err(e) = config.save(&cli.config_file, base_op).await {
        return CliResult {
            outcome: Err(e.context("Failed to write config file")),
            project_id: Some(project_id),
            telemetry_enabled: true,
        };
    }

    // Create the spawn folder structure
    let spawn_folder = &config.spawn_folder;
    let subfolders = ["migrations", "components", "tests", "pinned"];
    let mut created_folders = Vec::new();

    for subfolder in &subfolders {
        let path = format!("{}/{}/", spawn_folder, subfolder);
        // Create a .gitkeep file to ensure the folder exists
        if let Err(e) = base_op.write(&format!("{}.gitkeep", path), "").await {
            return CliResult {
                outcome: Err(anyhow::Error::from(e)
                    .context(format!("Failed to create {} folder", subfolder))),
                project_id: Some(project_id),
                telemetry_enabled: true,
            };
        }
        created_folders.push(format!("  {}{}/", spawn_folder, subfolder));
    }

    println!("Initialized spawn project with project_id: {}", project_id);
    println!("Created directories:");
    for folder in &created_folders {
        println!("{}", folder);
    }
    println!(
        "\nEdit {} to configure your database connection.",
        &cli.config_file
    );

    CliResult {
        outcome: Ok(Outcome::Success),
        project_id: Some(project_id),
        telemetry_enabled: true,
    }
}

async fn run_command(cli: Cli, mut main_config: Config, base_op: &Operator) -> Result<Outcome> {
    match &cli.command {
        Some(Commands::Init) => unreachable!(), // Already handled in run_cli
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
                    let mut pinner = Spawn::new(
                        main_config.pather().pinned_folder(),
                        main_config.pather().components_folder(),
                    )
                    .context("could not get pinned_folder")?;

                    let root = pinner
                        .snapshot(&main_config.operator())
                        .await
                        .context("error calling pinner snapshot")?;
                    let lock_file_path = main_config.pather().migration_lock_file_path(&migration);
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
                    let vars = match variables {
                        Some(vars_path) => {
                            Some(main_config.load_variables_from_path(vars_path).await?)
                        }
                        None => None,
                    };

                    let mgrtr = Migrator::new(&main_config, &migration, *pinned);
                    match mgrtr.generate_streaming(vars).await {
                        Ok(gen) => {
                            let mut buffer = Vec::new();
                            gen.render_to_writer(&mut buffer)
                                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
                            let content = String::from_utf8(buffer)?;
                            Ok(Outcome::BuiltMigration { content })
                        }
                        Err(e) => return Err(e),
                    }
                }
                Some(MigrationCommands::Apply {
                    migration,
                    pinned,
                    variables,
                }) => {
                    let vars = match variables {
                        Some(vars_path) => {
                            Some(main_config.load_variables_from_path(vars_path).await?)
                        }
                        None => None,
                    };

                    let mut migrations = Vec::new();
                    match migration {
                        Some(migration) => migrations.push(migration.clone()),
                        None => {
                            return Err(anyhow::anyhow!("applying all migrations not implemented"))
                        }
                    }

                    for migration in migrations {
                        let mgrtr = Migrator::new(&main_config, &migration, *pinned);
                        match mgrtr.generate_streaming(vars.clone()).await {
                            Ok(streaming) => {
                                let engine = main_config.new_engine().await?;
                                let write_fn = streaming.into_writer_fn();
                                match engine
                                    .migration_apply(&migration, write_fn, None, "default")
                                    .await
                                {
                                    Ok(_) => {
                                        println!("Migration '{}' applied successfully", &migration);
                                    }
                                    Err(MigrationError::AlreadyApplied { info, .. }) => {
                                        println!(
                                            "Migration '{}' already applied (status: {}, checksum: {})",
                                            &migration, info.last_status, info.checksum
                                        );
                                    }
                                    Err(MigrationError::PreviousAttemptFailed {
                                        status,
                                        info,
                                        ..
                                    }) => {
                                        return Err(anyhow::anyhow!(
                                            "Migration '{}' has a previous {} attempt (checksum: {}). \
                                             Manual intervention may be required.",
                                            &migration,
                                            status,
                                            info.checksum
                                        ));
                                    }
                                    Err(MigrationError::Database(e)) => {
                                        return Err(anyhow!(
                                            "Failed applying migration {}",
                                            &migration
                                        )
                                        .context(e));
                                    }
                                    Err(MigrationError::AdvisoryLock(e)) => {
                                        return Err(anyhow!(
                                            "Unable to obtain advisory lock for migration"
                                        )
                                        .context(e));
                                    }
                                    Err(
                                        e @ MigrationError::MigrationAppliedButNotRecorded {
                                            ..
                                        },
                                    ) => {
                                        // This is a critical error - the migration ran but wasn't recorded.
                                        // Return it directly so the full error message is displayed.
                                        return Err(anyhow::anyhow!("{}", e));
                                    }
                                }
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
                Ok(Outcome::Success)
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
                Ok(Outcome::Success)
            }
            Some(TestCommands::Compare { name }) => {
                let test_files: Vec<String> = match name {
                    Some(name) => vec![name.clone()],
                    None => {
                        let mut tests: Vec<String> = Vec::new();
                        let mut fs_lister = main_config
                            .operator()
                            .lister(&main_config.pather().tests_folder())
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

                Ok(Outcome::Success)
            }
            Some(TestCommands::Expect { name }) => {
                let tester = Tester::new(&main_config, &name);
                match tester.save_expected(None).await {
                    Ok(_) => (),
                    Err(e) => return Err(e),
                };
                Ok(Outcome::Success)
            }
            None => {
                eprintln!("No test subcommand specified");
                Ok(Outcome::Unimplemented)
            }
        },
        None => Ok(Outcome::Unimplemented),
    }
}
