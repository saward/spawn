use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::migrator::Migrator;
use crate::secrets::SecretsRenderMode;
use crate::variables::{resolve_declared, PromptMode, Variables};
use anyhow::Result;

pub struct BuildMigration {
    pub migration: String,
    pub pinned: bool,
    pub variables: Option<Variables>,
    /// Render real secret values instead of masked placeholders. Only
    /// intended for local debugging: the output of `build` is meant for
    /// inspection, not execution.
    pub reveal_secrets: bool,
    /// Take declared defaults instead of asking, for unattended builds.
    pub use_defaults: bool,
}

impl TelemetryDescribe for BuildMigration {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("migration build").with_properties(vec![
            ("opt_pinned", self.pinned.to_string()),
            ("has_variables", self.variables.is_some().to_string()),
            ("opt_reveal_secrets", self.reveal_secrets.to_string()),
            ("opt_use_defaults", self.use_defaults.to_string()),
        ])
    }
}

impl Command for BuildMigration {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let mgrtr = Migrator::resolve(config, &self.migration, self.pinned).await?;

        // Warn only when there is a pin being ignored. A lock.toml that exists
        // solely to declare variables pins nothing, so there is nothing to
        // warn about.
        let pinned_warn = !self.pinned && mgrtr.is_pinned();

        let secrets_mode = if self.reveal_secrets {
            SecretsRenderMode::Revealed
        } else {
            SecretsRenderMode::Masked
        };

        // A build renders what an apply would execute, so it needs the same
        // values and asks for them the same way. There is no confirmation to
        // skip here, so only the terminal decides. Prompts go to stderr,
        // leaving stdout pipeable.
        let declared = resolve_declared(
            mgrtr.declared_variables(),
            self.variables.as_ref(),
            PromptMode::resolve(self.use_defaults, false),
        )?;
        let variables = Variables::with_defaults(self.variables.clone(), &declared)?;

        match mgrtr.generate_streaming(variables, secrets_mode).await {
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
