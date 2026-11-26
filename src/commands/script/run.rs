use crate::commands::script::DEFAULT_NAMESPACE;
use crate::commands::{target_banner, Command, Outcome, TelemetryDescribe, TelemetryInfo};
use crate::config::Config;
use crate::engine::{MigrationStatus, ScriptOutput, ScriptStatus};
use crate::script::{PinMode, Script};
use crate::secrets::{redact_secrets, redact_text, SecretsRenderMode};
use crate::variables::{resolve_declared, PromptMode, Variables};
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
    /// Take declared defaults instead of asking, for unattended runs.
    pub use_defaults: bool,
}

impl TelemetryDescribe for RunScript {
    fn telemetry(&self) -> TelemetryInfo {
        TelemetryInfo::new("script run").with_properties(vec![
            (
                "opt_pin_mode",
                format!("{:?}", self.pin_mode).to_lowercase(),
            ),
            ("has_variables", self.variables.is_some().to_string()),
            ("opt_use_defaults", self.use_defaults.to_string()),
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
            eprintln!("Aborted.");
            return Ok(Outcome::Success);
        }

        // Asked for after the confirmation, so a run aimed at the wrong target
        // is abandoned before anything is typed into a prompt. `--yes` means
        // unattended, so it suppresses these as well as the confirmation.
        let declared = resolve_declared(
            script.declared_variables(),
            self.variables.as_ref(),
            PromptMode::resolve(self.use_defaults, self.yes),
        )?;
        let variables = Variables::with_defaults(self.variables.clone(), &declared)?;

        let dialect = crate::template::engine_to_dialect(&config.target_config()?.engine);

        // Running means executing real SQL, so secrets must be revealed.
        let streaming = script
            .generate_streaming(variables, SecretsRenderMode::Revealed)
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

        // Results go to stdout as they arrive. Diagnostics do not: psql can
        // quote a failing statement back, which may include a secret value.
        // They are dropped here; the engine keeps the last of them, and those
        // are printed once the render has finished, the first point at which
        // every secret value is known and can be removed. Results are never
        // redacted: a script that selects a secret asked for it.
        let mut results = tokio::io::stdout();
        let mut diagnostics = tokio::io::sink();

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

        if let Ok(run) = &outcome {
            if let Some(tail) = &run.diagnostics_tail {
                eprintln!("{}", redact_text(tail, &secrets, dialect));
            }
        }

        let status = match &outcome {
            Ok(run) if run.status == ScriptStatus::Ok => MigrationStatus::Success,
            _ => MigrationStatus::Failure,
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
            Ok(run) if run.status == ScriptStatus::Ok => {
                eprintln!(
                    "Script '{}' ran successfully in {:.2}s",
                    self.script, elapsed
                );
                Ok(Outcome::Success)
            }
            Ok(_) => Err(redact_secrets(
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
        eprintln!("{}", target_banner(config)?);
        eprintln!();
        eprintln!("The following script will be run:");
        eprintln!(
            "  {}{}",
            self.script,
            if use_pinned { " (pinned)" } else { "" }
        );
        eprintln!();

        if self.yes {
            return Ok(true);
        }

        // wait_for_newline so the Enter after `y` is consumed here rather than
        // left in the buffer, where the first variable prompt would read it as
        // an empty answer and silently take that variable's default.
        let confirmed = Confirm::new()
            .with_prompt("Do you want to run this script?")
            .default(false)
            .wait_for_newline(true)
            .interact()
            .context("could not ask for confirmation here: pass --yes to run unattended")?;
        eprintln!();

        Ok(confirmed)
    }
}
