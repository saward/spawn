use crate::commands::{
    AdoptMigration, ApplyMigration, BuildMigration, BuildTest, Check, Command, CompareTests,
    ExpectTest, Init, MigrationStatus, NewMigration, NewTest, Outcome, PinCleanup, PinMigration,
    PinVerify, RunTest, TelemetryDescribe, TelemetryInfo,
};
use crate::completions::{complete_migrations, complete_tests};
use crate::config::{Config, DEFAULT_CONFIG_FILE};
use opendal::Operator;

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use clap_complete::ArgValueCompleter;

#[derive(Parser)]
#[command(version, about, long_about = None)]
pub struct Cli {
    /// Turn debugging information on
    #[arg(short, long)]
    pub debug: bool,

    #[arg(global = true, short, long, default_value = DEFAULT_CONFIG_FILE)]
    pub config_file: String,

    #[arg(global = true, long)]
    pub target: Option<String>,

    /// Internal flag for telemetry child process (hidden)
    #[arg(long, hide = true)]
    pub internal_telemetry: bool,

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
    Init {
        /// Generate a docker-compose.yaml file for local PostgreSQL development.
        /// Optionally specify a database/project name (defaults to 'postgres').
        #[arg(long)]
        docker: Option<Option<String>>,
    },
    /// Check for potential issues (unpinned migrations, etc.)
    Check,
    Pin {
        #[command(subcommand)]
        command: Option<PinCommands>,
    },
    Migration {
        #[command(subcommand)]
        command: Option<MigrationCommands>,
        #[arg(short, long, global = true)]
        environment: Option<String>,
    },
    Test {
        #[command(subcommand)]
        command: Option<TestCommands>,
        #[arg(short, long, global = true)]
        environment: Option<String>,
    },
}

impl TelemetryDescribe for Commands {
    fn telemetry(&self) -> TelemetryInfo {
        match self {
            Commands::Init { .. } => TelemetryInfo::new("init"),
            Commands::Check => TelemetryInfo::new("check"),
            Commands::Pin { command } => match command {
                Some(cmd) => {
                    let mut info = cmd.telemetry();
                    info.label = format!("pin {}", info.label);
                    info
                }
                None => TelemetryInfo::new("pin"),
            },
            Commands::Migration { command, .. } => match command {
                Some(cmd) => {
                    let mut info = cmd.telemetry();
                    info.label = format!("migration {}", info.label);
                    info
                }
                None => TelemetryInfo::new("migration"),
            },
            Commands::Test { command, .. } => match command {
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
        #[arg(add = ArgValueCompleter::new(complete_migrations))]
        migration: String,
    },
    /// Build a migration into SQL
    Build {
        /// Whether to use pinned components
        #[arg(long)]
        pinned: bool,
        /// Migration to build.  Looks for up.sql inside this specified
        /// migration folder.
        #[arg(add = ArgValueCompleter::new(complete_migrations))]
        migration: String,
        /// Path to a variables file (JSON, TOML, or YAML) to use for templating.
        /// Overrides the variables_file setting in spawn.toml.
        #[arg(long)]
        variables: Option<String>,

        /// Render real secret() values instead of masked placeholders.
        /// Intended for local debugging only; build output is not executed.
        #[arg(long)]
        reveal_secrets: bool,
    },
    /// Apply will apply this migration to the database if not already applied,
    /// or all migrations if called without argument.
    Apply {
        /// Skip the pin requirement and use unpinned components
        #[arg(long)]
        no_pin: bool,

        #[arg(add = ArgValueCompleter::new(complete_migrations))]
        migration: Option<String>,

        /// Path to a variables file (JSON, TOML, or YAML) to use for templating.
        /// Overrides the variables_file setting in spawn.toml.
        #[arg(long)]
        variables: Option<String>,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,

        /// Retry a previously failed migration
        #[arg(long)]
        retry: bool,

        /// Reuse the same database connection across all migrations.
        /// Can significantly speed up applying many migrations.
        #[arg(long)]
        reuse_connection: bool,
    },
    /// Mark a migration as applied without actually running it.
    /// Useful when a migration was applied manually and needs to be recorded.
    Adopt {
        /// Migration to adopt
        #[arg(add = ArgValueCompleter::new(complete_migrations))]
        migration: Option<String>,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,

        /// Description of why the migration is being adopted
        #[arg(long)]
        description: Option<String>,
    },
    /// Show the status of all migrations
    Status,
}

impl TelemetryDescribe for MigrationCommands {
    fn telemetry(&self) -> TelemetryInfo {
        match self {
            MigrationCommands::New { .. } => TelemetryInfo::new("new"),
            MigrationCommands::Pin { .. } => TelemetryInfo::new("pin"),
            MigrationCommands::Build {
                pinned,
                variables,
                reveal_secrets,
                ..
            } => TelemetryInfo::new("build").with_properties(vec![
                ("opt_pinned", pinned.to_string()),
                ("has_variables", variables.is_some().to_string()),
                ("opt_reveal_secrets", reveal_secrets.to_string()),
            ]),
            MigrationCommands::Apply {
                no_pin,
                variables,
                migration,
                retry,
                reuse_connection,
                ..
            } => TelemetryInfo::new("apply").with_properties(vec![
                ("opt_no_pin", no_pin.to_string()),
                ("opt_retry", retry.to_string()),
                ("has_variables", variables.is_some().to_string()),
                ("apply_all", migration.is_none().to_string()),
                ("opt_reuse_connection", reuse_connection.to_string()),
            ]),
            MigrationCommands::Adopt { .. } => TelemetryInfo::new("adopt"),
            MigrationCommands::Status => TelemetryInfo::new("status"),
        }
    }
}

#[derive(Subcommand)]
pub enum PinCommands {
    /// Clean up orphaned pinned files that are no longer referenced by any migration
    Cleanup {
        /// Show what would be deleted without actually deleting
        #[arg(long)]
        dry_run: bool,
    },
    /// Verify the integrity of the pinned store: that every pinned file still
    /// matches its content hash, and that every migration's lock.toml points
    /// to a root hash that exists in the store.
    Verify,
}

impl TelemetryDescribe for PinCommands {
    fn telemetry(&self) -> TelemetryInfo {
        match self {
            PinCommands::Cleanup { dry_run } => TelemetryInfo::new("cleanup")
                .with_properties(vec![("dry_run", dry_run.to_string())]),
            PinCommands::Verify => TelemetryInfo::new("verify"),
        }
    }
}

#[derive(Subcommand)]
pub enum TestCommands {
    /// Create a new test with the provided name
    New {
        /// Name of the test
        name: String,
    },
    Build {
        #[arg(add = ArgValueCompleter::new(complete_tests))]
        name: String,

        /// Render real secret() values instead of masked placeholders.
        /// Intended for local debugging only; build output is not executed.
        #[arg(long)]
        reveal_secrets: bool,
    },
    /// Run a particular test, or all tests if no name provided.
    Run {
        #[arg(add = ArgValueCompleter::new(complete_tests))]
        name: Option<String>,
    },
    /// Run tests and compare to expected.  Runs all tests if no name provided.
    Compare {
        #[arg(add = ArgValueCompleter::new(complete_tests))]
        name: Option<String>,
    },
    Expect {
        #[arg(add = ArgValueCompleter::new(complete_tests))]
        name: String,
    },
}

impl TelemetryDescribe for TestCommands {
    fn telemetry(&self) -> TelemetryInfo {
        match self {
            TestCommands::New { .. } => TelemetryInfo::new("new"),
            TestCommands::Build { reveal_secrets, .. } => TelemetryInfo::new("build")
                .with_properties(vec![("opt_reveal_secrets", reveal_secrets.to_string())]),
            TestCommands::Run { name } => TelemetryInfo::new("run")
                .with_properties(vec![("run_all", name.is_none().to_string())]),
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
    if let Some(Commands::Init { docker }) = &cli.command {
        let init_cmd = Init {
            config_file: cli.config_file.clone(),
            docker: docker.clone(),
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

    // Check if config file exists to show telemetry notice
    let config_exists = base_op.exists(&cli.config_file).await.unwrap_or(false);

    // Load config from file (required for all other commands)
    let mut main_config = match Config::load(&cli.config_file, base_op, cli.target.clone()).await {
        Ok(cfg) => cfg,
        Err(e) => {
            // If config doesn't exist, show helpful message
            if !config_exists {
                crate::show_telemetry_notice();
                eprintln!("No spawn.toml configuration file found.");
                eprintln!("Run `spawn init` to create a new spawn project.");
                return CliResult {
                    outcome: Err(anyhow!("Configuration file not found")),
                    project_id: None,
                    telemetry_enabled: false,
                };
            }

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
        Some(Commands::Init { .. }) => unreachable!(), // Already handled in run_cli
        Some(Commands::Check) => Check.execute(config).await,
        Some(Commands::Pin { command }) => match command {
            Some(PinCommands::Cleanup { dry_run }) => PinCleanup { dry_run }.execute(config).await,
            Some(PinCommands::Verify) => PinVerify.execute(config).await,
            None => {
                eprintln!("No pin subcommand specified");
                Ok(Outcome::Unimplemented)
            }
        },
        Some(Commands::Migration {
            command,
            environment,
        }) => {
            // Only override what was loaded from spawn.toml when --environment
            // was actually passed; otherwise preserve an explicit top-level
            // `environment` setting (or the target's own) instead of erasing it.
            if let Some(environment) = environment {
                config.environment = Some(environment);
            }
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
                    reveal_secrets,
                }) => {
                    let vars = match variables {
                        Some(vars_path) => Some(config.load_variables_from_path(&vars_path).await?),
                        None => None,
                    };
                    BuildMigration {
                        migration,
                        pinned,
                        variables: vars,
                        reveal_secrets,
                    }
                    .execute(config)
                    .await
                }
                Some(MigrationCommands::Apply {
                    migration,
                    no_pin,
                    variables,
                    yes,
                    retry,
                    reuse_connection,
                }) => {
                    let vars = match variables {
                        Some(vars_path) => Some(config.load_variables_from_path(&vars_path).await?),
                        None => None,
                    };
                    ApplyMigration {
                        migration,
                        pinned: !no_pin,
                        variables: vars,
                        yes,
                        retry,
                        reuse_connection,
                    }
                    .execute(config)
                    .await
                }
                Some(MigrationCommands::Adopt {
                    migration,
                    yes,
                    description,
                }) => {
                    AdoptMigration {
                        migration,
                        yes,
                        description,
                    }
                    .execute(config)
                    .await
                }
                Some(MigrationCommands::Status) => MigrationStatus.execute(config).await,
                None => {
                    eprintln!("No migration subcommand specified");
                    Ok(Outcome::Unimplemented)
                }
            }
        }
        Some(Commands::Test {
            command,
            environment,
        }) => {
            if let Some(environment) = environment {
                config.environment = Some(environment);
            }
            match command {
                Some(TestCommands::New { name }) => NewTest { name }.execute(config).await,
                Some(TestCommands::Build {
                    name,
                    reveal_secrets,
                }) => {
                    BuildTest {
                        name,
                        reveal_secrets,
                    }
                    .execute(config)
                    .await
                }
                Some(TestCommands::Run { name }) => RunTest { name }.execute(config).await,
                Some(TestCommands::Compare { name }) => {
                    CompareTests { name }.execute(config).await
                }
                Some(TestCommands::Expect { name }) => ExpectTest { name }.execute(config).await,
                None => {
                    eprintln!("No test subcommand specified");
                    Ok(Outcome::Unimplemented)
                }
            }
        }
        None => Ok(Outcome::Unimplemented),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigLoaderSaver;
    use crate::engine::{EngineType, TargetConfig};
    use opendal::services::Memory;
    use std::collections::HashMap;

    /// A config with a target permanently set to "dev", and an optional
    /// explicit top-level `environment` override (`None` reflects the
    /// common case: nothing set in spawn.toml).
    fn test_config(top_level_environment: Option<&str>) -> Config {
        let mut targets = HashMap::new();
        targets.insert(
            "dev_target".to_string(),
            TargetConfig {
                engine: EngineType::PostgresPSQL,
                spawn_database: None,
                spawn_schema: "_spawn".to_string(),
                environment: "dev".to_string(),
                command: None,
            },
        );
        let loader = ConfigLoaderSaver {
            environment: top_level_environment.map(|s| s.to_string()),
            project_id: None,
            spawn_folder: "spawn".to_string(),
            target: Some("dev_target".to_string()),
            targets: Some(targets),
            secrets: None,
            template_test: None,
            template_up: None,
            telemetry: Some(false),
        };
        let op = opendal::Operator::new(Memory::default()).unwrap();
        loader.build(op, None)
    }

    /// Table-driven coverage of how a target's environment interacts with
    /// spawn.toml's top-level `environment` and the `--environment` CLI
    /// flag, for both Migration and Test commands.
    #[tokio::test]
    async fn resolved_environment_matrix() {
        struct Case {
            name: &'static str,
            is_test_command: bool,
            top_level_environment: Option<&'static str>,
            cli_flag: Option<&'static str>,
            expected: &'static str,
        }

        let cases = [
            Case {
                name: "test: no config or flag falls back to the target's own environment",
                is_test_command: true,
                top_level_environment: None,
                cli_flag: None,
                expected: "dev",
            },
            Case {
                name: "test: --environment flag overrides the target",
                is_test_command: true,
                top_level_environment: None,
                cli_flag: Some("staging"),
                expected: "staging",
            },
            Case {
                name: "test: explicit top-level override is preserved when flag is absent",
                is_test_command: true,
                top_level_environment: Some("staging"),
                cli_flag: None,
                expected: "staging",
            },
            Case {
                name: "migration: no config or flag falls back to the target's own environment",
                is_test_command: false,
                top_level_environment: None,
                cli_flag: None,
                expected: "dev",
            },
            Case {
                name: "migration: --environment flag overrides the target",
                is_test_command: false,
                top_level_environment: None,
                cli_flag: Some("staging"),
                expected: "staging",
            },
            Case {
                name: "migration: explicit top-level override is preserved when flag is absent",
                is_test_command: false,
                top_level_environment: Some("staging"),
                cli_flag: None,
                expected: "staging",
            },
        ];

        for case in cases {
            let mut config = test_config(case.top_level_environment);
            let environment = case.cli_flag.map(String::from);
            let command = if case.is_test_command {
                Commands::Test {
                    command: None,
                    environment,
                }
            } else {
                Commands::Migration {
                    command: None,
                    environment,
                }
            };
            let cli = Cli {
                debug: false,
                config_file: "spawn.toml".to_string(),
                target: None,
                internal_telemetry: false,
                command: Some(command),
            };

            run_command(cli, &mut config).await.unwrap();
            assert_eq!(
                config.target_config().unwrap().environment,
                case.expected,
                "case: {}",
                case.name
            );
        }
    }
}
