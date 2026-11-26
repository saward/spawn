use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::script::{PinMode, Script};
use crate::secrets::SecretsRenderMode;
use crate::variables::{resolve_declared, PromptMode, Variables};
use anyhow::{anyhow, Result};

pub struct BuildScript {
    /// Path of the script to build, relative to `scripts/`.
    pub script: String,
    /// Which component tree the includes resolve against.
    pub pin_mode: PinMode,
    pub variables: Option<Variables>,
    /// Render real secret values instead of masked placeholders; local debugging only.
    pub reveal_secrets: bool,
    /// Take declared defaults instead of asking, for unattended builds.
    pub use_defaults: bool,
}

impl TelemetryDescribe for BuildScript {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("script build").with_properties(vec![
            (
                "opt_pin_mode",
                format!("{:?}", self.pin_mode).to_lowercase(),
            ),
            ("has_variables", self.variables.is_some().to_string()),
            ("opt_reveal_secrets", self.reveal_secrets.to_string()),
            ("opt_use_defaults", self.use_defaults.to_string()),
        ])
    }
}

impl Command for BuildScript {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let script = Script::new(config, &self.script, false);
        if !script.exists().await? {
            return Err(anyhow!(
                "script '{}' not found at '{}'",
                self.script,
                config.pather().script_file_path(&self.script)
            ));
        }
        let script = Script::resolve(config, &self.script, self.pin_mode).await?;
        eprintln!("{}", script.pin_note());

        let secrets_mode = if self.reveal_secrets {
            SecretsRenderMode::Revealed
        } else {
            SecretsRenderMode::Masked
        };

        // A build renders what a run would execute, so it needs the same values
        // and asks for them the same way. There is no confirmation to skip
        // here, so only the terminal decides. Prompts go to stderr, leaving
        // stdout pipeable.
        let declared = resolve_declared(
            script.declared_variables(),
            self.variables.as_ref(),
            PromptMode::resolve(self.use_defaults, false),
        )?;
        let variables = Variables::with_defaults(self.variables.clone(), &declared)?;

        let gen = script.generate_streaming(variables, secrets_mode).await?;

        let mut buffer = Vec::new();
        gen.render_to_writer(&mut buffer)
            .map_err(std::io::Error::other)?;

        Ok(Outcome::BuiltScript {
            content: String::from_utf8(buffer)?,
        })
    }
}
