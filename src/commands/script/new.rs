use crate::commands::{Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::script::Script;
use anyhow::{anyhow, Context, Result};

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

/// Rejects a name whose folder would not be `scripts/<name>/`. Commands that
/// walk `scripts/` could not then find the script, so `spawn pin cleanup` would
/// treat its pinned trees as orphaned and delete them.
fn validate_script_path(script: &str) -> Result<()> {
    if script.split('/').any(|seg| matches!(seg, "" | "." | "..")) {
        return Err(anyhow!(
            "invalid script name '{}': must be a relative path with no empty, '.' or '..' segments",
            script
        ));
    }
    Ok(())
}

impl Command for NewScript {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        validate_script_path(&self.script)?;

        let script = Script::new(config, &self.script, false);

        // Names are chosen rather than timestamped, so overwriting could
        // clobber a script someone has already run.
        if script.exists().await? {
            return Err(anyhow!("script '{}' already exists", self.script));
        }

        let template: Option<String> = match &config.template_script {
            Some(t) => {
                let template_path = config.pather().any_path(t);
                let content = config
                    .operator()
                    .read(&template_path)
                    .await
                    .context(format!(
                        "Failed to read script template file '{}'",
                        &template_path
                    ))?
                    .to_bytes();
                Some(
                    String::from_utf8(content.to_vec())
                        .context("Script template file is not valid UTF-8")?,
                )
            }
            None => None,
        };

        let path = config.pather().script_file_path(&self.script);
        config
            .operator()
            .write(&path, template.unwrap_or_else(|| BASE_SCRIPT.to_string()))
            .await?;

        Ok(Outcome::NewScript {
            script: self.script.clone(),
            path,
        })
    }
}
