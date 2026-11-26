use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::script::Script;
use crate::source_config::write_pin;
use crate::store::pinner::spawn::Spawn;
use crate::store::pinner::Pinner;
use anyhow::{Context, Result};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PinScriptError {
    #[error("script '{0}' does not exist")]
    ScriptNotFound(String),
}

pub struct PinScript {
    /// Path of the script to pin, relative to `scripts/`.
    pub script: String,
}

impl TelemetryDescribe for PinScript {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("script pin")
    }
}

impl Command for PinScript {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let script = Script::new(config, &self.script, false);
        if !script.exists().await? {
            return Err(PinScriptError::ScriptNotFound(self.script.clone()).into());
        }

        let mut pinner = Spawn::new(
            config.pather().pinned_folder(),
            config.pather().components_folder(),
        )
        .context("could not get pinned_folder")?;

        // Snapshots the whole component tree, as migration pinning does; the
        // script itself lives under `scripts/` and is never pinned.
        let root = pinner
            .snapshot(config.operator())
            .await
            .context("error calling pinner snapshot")?;

        // Sets only the `pin` key: config.toml may already declare variables,
        // which pinning has no business discarding or reformatting.
        let config_path = config.pather().script_config_path(&self.script);
        write_pin(config.operator(), &config_path, &root)
            .await
            .context("failed writing script config")?;

        Ok(Outcome::PinnedScript {
            script: self.script.clone(),
            hash: root,
        })
    }
}
