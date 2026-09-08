use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::migrator::Migrator;
use crate::secrets::SecretsRenderMode;
use crate::store::get_migration_fs_status;
use crate::variables::Variables;
use anyhow::Result;

pub struct BuildMigration {
    pub migration: String,
    pub pinned: bool,
    pub variables: Option<Variables>,
    /// Render real secret values instead of masked placeholders. Only
    /// intended for local debugging: the output of `build` is meant for
    /// inspection, not execution.
    pub reveal_secrets: bool,
}

impl TelemetryDescribe for BuildMigration {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("migration build").with_properties(vec![
            ("opt_pinned", self.pinned.to_string()),
            ("has_variables", self.variables.is_some().to_string()),
            ("opt_reveal_secrets", self.reveal_secrets.to_string()),
        ])
    }
}

impl Command for BuildMigration {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let mgrtr = Migrator::new(config, &self.migration, self.pinned);

        // Check if lock file exists when not using --pinned
        let pinned_warn = if !self.pinned {
            let fs_status =
                get_migration_fs_status(config.operator(), &config.pather(), &self.migration)
                    .await?;
            fs_status.has_lock_toml
        } else {
            false
        };

        let secrets_mode = if self.reveal_secrets {
            SecretsRenderMode::Revealed
        } else {
            SecretsRenderMode::Masked
        };

        match mgrtr
            .generate_streaming(self.variables.clone(), secrets_mode)
            .await
        {
            Ok(gen) => {
                let mut buffer = Vec::new();
                gen.render_to_writer(&mut buffer)
                    .map_err(std::io::Error::other)?;
                let content = String::from_utf8(buffer)?;

                Ok(Outcome::BuiltMigration {
                    content,
                    pinned_warn,
                })
            }
            Err(e) => Err(e),
        }
    }
}
