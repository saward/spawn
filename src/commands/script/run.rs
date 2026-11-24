use crate::commands::script::DEFAULT_NAMESPACE;
use crate::commands::{print_target_banner, Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::engine::{MigrationStatus, ScriptOutput, ScriptStatus};
use crate::script::{PinMode, Script};
use crate::secrets::{redact_secrets, SecretsRenderMode};
use crate::variables::Variables;
use anyhow::{anyhow, Context, Result};
use dialoguer::Confirm;
use std::time::Instant;

pub struct RunScript {
    /// Path of the script to run, relative to `scripts/`.
    pub script: String,
    /// Which component tree the includes resolve against.
    pub pin_mode: PinMode,
    pub variables: Option<Variables>,
    pub yes: bool,
}

impl TelemetryDescribe for RunScript {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("script run").with_properties(vec![
            (
                "opt_pin_mode",
                format!("{:?}", self.pin_mode).to_lowercase(),
            ),
            ("has_variables", self.variables.is_some().to_string()),
        ])
    }
}

impl Command for RunScript {
    async fn execute(&self, config: &Config) -> Result<Outcome> {
        // Resolved before the existence check so that `--pinned` against a
        // script with neither file still reports the missing script first.
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

        if !self.confirm(config, script.use_pinned())? {
            println!("Aborted.");
            return Ok(Outcome::Success);
        }

        let dialect = crate::template::engine_to_dialect(&config.target_config()?.engine);

        // Running means executing real SQL, so secrets must be revealed.
        let streaming = script
            .generate_streaming(self.variables.clone(), SecretsRenderMode::Revealed)
            .await
            .with_context(|| format!("failed to generate script '{}'", self.script))?;

        // Fingerprint the raw template source, never the rendered output, which
        // may contain resolved secret values.
        let checksum = streaming.raw_checksum();
        // Prefer the pin this render used over re-reading config.toml, which
        // could race a concurrent re-pin; an unpinned run recomputes one.
        let pin_hash = match streaming.pin_hash() {
            Some(hash) => hash.to_string(),
            None => script.recompute_pin_hash().await?,
        };

        let engine = config.new_engine().await?;
        let (script, secrets) = streaming.into_script_source();

        // Streamed rather than collected: a long run should show output as it
        // happens, on separate streams so stdout stays pipeable. The diagnostics
        // are psql's own and unredacted — a failing statement can echo back a
        // revealed secret.
        let mut results = tokio::io::stdout();
        let mut diagnostics = tokio::io::stderr();

        let started = Instant::now();
        let outcome = engine
            .run_script(
                script,
                ScriptOutput {
                    results: &mut results,
                    diagnostics: &mut diagnostics,
                },
            )
            .await;
        let elapsed = started.elapsed().as_secs_f32();

        let status = match &outcome {
            Ok(ScriptStatus::Ok) => MigrationStatus::Success,
            Ok(ScriptStatus::StatementFailed) | Err(_) => MigrationStatus::Failure,
        };

        // Recorded before any failure is surfaced: the audit trail matters most
        // for the run that went wrong.
        if let Err(record_err) = engine
            .record_script_run(
                &self.script,
                checksum,
                Some(pin_hash),
                DEFAULT_NAMESPACE,
                status,
                elapsed,
            )
            .await
        {
            // A warning, not an error: nothing about a later run depends on this row.
            eprintln!(
                "\u{26a0}\u{fe0f}  Script '{}' ran ({}), but the run could not be recorded: {}",
                self.script, status, record_err
            );
        }

        match outcome {
            Ok(ScriptStatus::Ok) => {
                eprintln!(
                    "Script '{}' ran successfully in {:.2}s",
                    self.script, elapsed
                );
                Ok(Outcome::Success)
            }
            Ok(ScriptStatus::StatementFailed) => Err(redact_secrets(
                anyhow!(
                    "script '{}' failed: a statement reported an error (see the output above)",
                    self.script
                ),
                &secrets,
                dialect,
            )),
            Err(e) => Err(redact_secrets(
                anyhow!("could not run script '{}': {}", self.script, e),
                &secrets,
                dialect,
            )),
        }
    }
}

impl RunScript {
    /// Asks whether to proceed; `Ok(true)` means go ahead.
    fn confirm(&self, config: &Config, use_pinned: bool) -> Result<bool> {
        if self.yes {
            return Ok(true);
        }

        print_target_banner(config)?;
        println!();
        println!("The following script will be run:");
        println!(
            "  {}{}",
            self.script,
            if use_pinned { " (pinned)" } else { "" }
        );
        println!();

        let confirmed = Confirm::new()
            .with_prompt("Do you want to run this script?")
            .default(false)
            .interact()?;
        println!();

        Ok(confirmed)
    }
}
