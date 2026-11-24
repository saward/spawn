use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::script::Script;
use anyhow::{anyhow, Result};

/// Starting point for a new script. Owns its own transaction, like
/// `BASE_MIGRATION`; scripts needing `CREATE INDEX CONCURRENTLY` or `VACUUM`
/// delete these lines.
static BASE_SCRIPT: &str = "BEGIN;

COMMIT;
";

pub struct NewScript {
    /// Path of the script to create, relative to `scripts/`. May nest.
    pub script: String,
}

impl TelemetryDescribe for NewScript {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("script new")
    }
}

impl Command for NewScript {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        let script = Script::new(config, &self.script, false);

        // Names are chosen rather than timestamped, so overwriting could
        // clobber a script someone has already run.
        if script.exists().await? {
            return Err(anyhow!("script '{}' already exists", self.script));
        }

        let path = config.pather().script_file_path(&self.script);
        config.operator().write(&path, BASE_SCRIPT).await?;

        Ok(Outcome::NewScript {
            script: self.script.clone(),
            path,
        })
    }
}
