use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::script::{PinMode, Script};
use crate::secrets::SecretsRenderMode;
use crate::variables::Variables;
use anyhow::{anyhow, Result};

pub struct BuildScript {
    /// Path of the script to build, relative to `scripts/`.
    pub script: String,
    /// Which component tree the includes resolve against.
    pub pin_mode: PinMode,
    pub variables: Option<Variables>,
    /// Render real secret values instead of masked placeholders; local debugging only.
    pub reveal_secrets: bool,
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

        let gen = script
            .generate_streaming(self.variables.clone(), secrets_mode)
            .await?;

        let mut buffer = Vec::new();
        gen.render_to_writer(&mut buffer)
            .map_err(std::io::Error::other)?;

        Ok(Outcome::BuiltScript {
            content: String::from_utf8(buffer)?,
        })
    }
}
