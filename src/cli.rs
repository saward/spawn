use crate::commands::{
    ApplyMigration, BuildMigration, BuildTest, Command, CompareTests, ExpectTest, Init,
    NewMigration, Outcome, PinMigration, RunTest, TelemetryDescribe, TelemetryInfo,
};
use crate::config::Config;
use opendal::Operator;

use anyhow::Result;
use clap::{Parser, Subcommand};

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
    fn telemetry(&self) -> TelemetryInfo {
        match &self.command {
            Some(cmd) => cmd.telemetry(),
            None => TelemetryInfo::default(),
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
    fn telemetry(&self) -> TelemetryInfo {
        match self {
            Commands::Init => TelemetryInfo::new("init"),
            Commands::Migration { command, .. } => match command {
                Some(cmd) => {
                    let mut info = cmd.telemetry();
                    info.label = format!("migration {}", info.label);
                    info
                }
                None => TelemetryInfo::new("migration"),
            },
            Commands::Test { command } => match command {
                Some(cmd) => {
                    let mut info = cmd.telemetry();
                    info.label = format!("test {}", info.label);
                    info
                }
                None => TelemetryInfo::new("test"),
            },
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
    fn telemetry(&self) -> TelemetryInfo {
        match self {
            MigrationCommands::New { .. } => TelemetryInfo::new("new"),
            MigrationCommands::Pin { .. } => TelemetryInfo::new("pin"),
            MigrationCommands::Build {
                pinned, variables, ..
            } => TelemetryInfo::new("build").with_properties(vec![
                ("opt_pinned", pinned.to_string()),
                ("has_variables", variables.is_some().to_string()),
            ]),
            MigrationCommands::Apply {
                pinned,
                variables,
                migration,
            } => TelemetryInfo::new("apply").with_properties(vec![
                ("opt_pinned", pinned.to_string()),
                ("has_variables", variables.is_some().to_string()),
                ("apply_all", migration.is_none().to_string()),
            ]),
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
    fn telemetry(&self) -> TelemetryInfo {
        match self {
            TestCommands::Build { .. } => TelemetryInfo::new("build"),
            TestCommands::Run { .. } => TelemetryInfo::new("run"),
            TestCommands::Compare { name } => TelemetryInfo::new("compare")
                .with_properties(vec![("compare_all", name.is_none().to_string())]),
            TestCommands::Expect { .. } => TelemetryInfo::new("expect"),
        }
    }
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
        let init_cmd = Init {
            config_file: cli.config_file.clone(),
        };
        match init_cmd.execute(base_op).await {
            Ok((outcome, project_id)) => {
                return CliResult {
                    outcome: Ok(outcome),
                    project_id: Some(project_id),
                    telemetry_enabled: true,
                };
            }
            Err(e) => {
                return CliResult {
                    outcome: Err(e),
                    project_id: None,
                    telemetry_enabled: true,
                };
            }
        }
    }

    // Load config from file (required for all other commands)
    let mut main_config = match Config::load(&cli.config_file, base_op, cli.database.clone()).await
    {
        Ok(cfg) => cfg,
        Err(e) => {
            return CliResult {
                outcome: Err(e.context(format!("could not load config from {}", &cli.config_file))),
                project_id: None,
                telemetry_enabled: false, // Default disabled if we can't load config
            };
        }
    };

    // Extract telemetry info from config
    let project_id = main_config.project_id.clone();
    let telemetry_enabled = main_config.telemetry;

    // Run the actual command
    let outcome = run_command(cli, &mut main_config).await;

    CliResult {
        outcome,
        project_id,
        telemetry_enabled,
    }
}

async fn run_command(cli: Cli, config: &mut Config) -> Result<Outcome> {
    match cli.command {
        Some(Commands::Init) => unreachable!(), // Already handled in run_cli
        Some(Commands::Migration {
            command,
            environment,
        }) => {
            config.environment = environment;
            match command {
                Some(MigrationCommands::New { name }) => {
                    NewMigration { name }.execute(config).await
                }
                Some(MigrationCommands::Pin { migration }) => {
                    PinMigration { migration }.execute(config).await
                }
                Some(MigrationCommands::Build {
                    migration,
                    pinned,
                    variables,
                }) => {
                    let vars = match variables {
                        Some(vars_path) => Some(config.load_variables_from_path(&vars_path).await?),
                        None => None,
                    };
                    BuildMigration {
                        migration,
                        pinned,
                        variables: vars,
                    }
                    .execute(config)
                    .await
                }
                Some(MigrationCommands::Apply {
                    migration,
                    pinned,
                    variables,
                }) => {
                    let vars = match variables {
                        Some(vars_path) => Some(config.load_variables_from_path(&vars_path).await?),
                        None => None,
                    };
                    ApplyMigration {
                        migration,
                        pinned,
                        variables: vars,
                    }
                    .execute(config)
                    .await
                }
                None => {
                    eprintln!("No migration subcommand specified");
                    Ok(Outcome::Unimplemented)
                }
            }
        }
        Some(Commands::Test { command }) => match command {
            Some(TestCommands::Build { name }) => BuildTest { name }.execute(config).await,
            Some(TestCommands::Run { name }) => RunTest { name }.execute(config).await,
            Some(TestCommands::Compare { name }) => CompareTests { name }.execute(config).await,
            Some(TestCommands::Expect { name }) => ExpectTest { name }.execute(config).await,
            None => {
                eprintln!("No test subcommand specified");
                Ok(Outcome::Unimplemented)
            }
        },
        None => Ok(Outcome::Unimplemented),
    }
}
