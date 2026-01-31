use crate::commands::{Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::ConfigLoaderSaver;
use crate::engine::{CommandSpec, DatabaseConfig, EngineType};
use anyhow::{anyhow, Result};
use opendal::Operator;
use std::collections::HashMap;
use uuid::Uuid;

/// Init command - special case that doesn't implement Command trait
/// because it doesn't require a loaded Config (it creates the config).
pub struct Init {
    pub config_file: String,
    /// Optional docker-compose generation. If Some(None), uses default name "myproject".
    /// If Some(Some(name)), uses the provided name.
    pub docker: Option<Option<String>>,
}

impl TelemetryDescribe for Init {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("init")
    }
}

impl Init {
    /// Execute the init command. Returns (Outcome, project_id).
    /// Unlike other commands, this takes an Operator directly since Config doesn't exist yet.
    pub async fn execute(&self, base_op: &Operator) -> Result<(Outcome, String)> {
        // Check if spawn.toml already exists
        if base_op.exists(&self.config_file).await? {
            return Err(anyhow!(
                "Config file '{}' already exists. Use a different path or remove the existing file.",
                &self.config_file
            ));
        }

        // Generate a new project_id
        let project_id = Uuid::new_v4().to_string();

        // Determine database/project name for docker setup
        let db_name = match &self.docker {
            Some(Some(name)) => name.clone(),
            Some(None) => "postgres".to_string(),
            None => "postgres".to_string(),
        };

        // Determine container name
        let container_name = if self.docker.is_some() {
            format!("{}-db", db_name)
        } else {
            "postgres-db".to_string()
        };

        // Create example database config
        let mut databases = HashMap::new();
        databases.insert(
            "postgres_psql".to_string(),
            DatabaseConfig {
                engine: EngineType::PostgresPSQL,
                spawn_database: db_name.clone(),
                spawn_schema: "_spawn".to_string(),
                environment: "dev".to_string(),
                command: Some(CommandSpec::Direct {
                    direct: vec![
                        "docker".to_string(),
                        "exec".to_string(),
                        "-i".to_string(),
                        container_name.clone(),
                        "psql".to_string(),
                        "-U".to_string(),
                        "postgres".to_string(),
                        db_name.clone(),
                    ],
                }),
            },
        );

        // Create default config
        let config = ConfigLoaderSaver {
            spawn_folder: "spawn".to_string(),
            database: Some("postgres_psql".to_string()),
            environment: None,
            databases: Some(databases),
            project_id: Some(project_id.clone()),
            telemetry: None,
        };

        // Save the config
        config
            .save(&self.config_file, base_op)
            .await
            .map_err(|e| e.context("Failed to write config file"))?;

        // Create the spawn folder structure
        let spawn_folder = &config.spawn_folder;
        let subfolders = ["migrations", "components", "tests", "pinned"];
        let mut created_folders = Vec::new();

        for subfolder in &subfolders {
            let path = format!("{}/{}/", spawn_folder, subfolder);
            // Create a .gitkeep file to ensure the folder exists
            base_op
                .write(&format!("{}.gitkeep", path), "")
                .await
                .map_err(|e| {
                    anyhow::Error::from(e).context(format!("Failed to create {} folder", subfolder))
                })?;
            created_folders.push(format!("  {}/{}/", spawn_folder, subfolder));
        }

        // Generate docker-compose.yaml if requested
        if self.docker.is_some() {
            let docker_compose_content = format!(
                r#"services:
  postgres:
    image: postgres:17
    container_name: {}
    ports:
      - "5432:5432"
    restart: always
    environment:
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: postgres
      POSTGRES_DB: {}
"#,
                container_name, db_name
            );

            base_op
                .write("docker-compose.yaml", docker_compose_content)
                .await
                .map_err(|e| {
                    anyhow::Error::from(e).context("Failed to create docker-compose.yaml")
                })?;

            println!("Created docker-compose.yaml for database '{}'", db_name);
            println!("Start the database with: docker compose up -d");
            println!();
        }

        // Show telemetry notice
        crate::show_telemetry_notice();

        println!("Initialized spawn project with project_id: {}", project_id);
        println!("Created directories:");
        for folder in &created_folders {
            println!("{}", folder);
        }
        println!(
            "\nEdit {} to configure your database connection.",
            &self.config_file
        );

        Ok((Outcome::Success, project_id))
    }
}
