use crate::commands::{Outcome, TelemetryDescribe};
use crate::config::ConfigLoaderSaver;
use anyhow::{anyhow, Result};
use opendal::Operator;
use uuid::Uuid;

/// Init command - special case that doesn't implement Command trait
/// because it doesn't require a loaded Config (it creates the config).
pub struct Init {
    pub config_file: String,
}

impl TelemetryDescribe for Init {
    fn telemetry_command(&self) -> String {
        "init".to_string()
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
            created_folders.push(format!("  {}{}/", spawn_folder, subfolder));
        }

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
