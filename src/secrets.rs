use anyhow::{anyhow, Context, Result};
use opendal::Operator;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// Where a secret's value is actually read from.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum SecretSource {
    /// Read from an OS environment variable.
    Env { name: String },
    /// Read a file via spawn's configured operator (trailing newline
    /// stripped), resolved the same way any other file spawn reads is. Not
    /// a real filesystem path — use `host_file` for that.
    File { path: String },
    /// Read a file directly from the host filesystem (trailing newline
    /// stripped), bypassing the operator. For secrets mounted on the host
    /// outside spawn's storage, e.g. Docker/Kubernetes secrets under
    /// `/run/secrets`, systemd's `LoadCredential=`, etc.
    HostFile { path: String },
    /// Run a command and use its trimmed stdout as the value.
    Command { command: Vec<String> },
    /// An inline value. Requires `insecure = true`, so a plaintext secret
    /// can't quietly end up as a "secure" default or environment override.
    Literal {
        value: String,
        #[serde(default)]
        insecure: bool,
    },
}

impl SecretSource {
    /// Fetches the value this source points to. Called even when the render
    /// will mask the result, so that `build`/`test build` still verify a
    /// secret is reachable (and that a `literal` source is marked
    /// `insecure`) without ever displaying the real value.
    async fn resolve(&self, operator: &Operator) -> Result<String> {
        match self {
            SecretSource::Env { name } => std::env::var(name)
                .with_context(|| format!("environment variable '{}' is not set", name)),
            SecretSource::File { path } => {
                let bytes = operator
                    .read(path)
                    .await
                    .with_context(|| format!("could not read secret file '{}'", path))?
                    .to_bytes();
                let text =
                    String::from_utf8(bytes.to_vec()).context("secret file is not valid UTF-8")?;
                Ok(text.trim_end_matches(['\n', '\r']).to_string())
            }
            SecretSource::HostFile { path } => {
                let bytes = tokio::fs::read(path)
                    .await
                    .with_context(|| format!("could not read secret host file '{}'", path))?;
                let text =
                    String::from_utf8(bytes).context("secret host file is not valid UTF-8")?;
                Ok(text.trim_end_matches(['\n', '\r']).to_string())
            }
            SecretSource::Command { command } => crate::engine::run_capture_stdout(command).await,
            SecretSource::Literal { value, insecure } => {
                if !insecure {
                    return Err(anyhow!(
                        "literal secret values must set 'insecure = true' to be used"
                    ));
                }
                Ok(value.clone())
            }
        }
    }
}

/// A secret's default source, with optional per-environment overrides.
///
/// `default` should be a source that's safe to fall back to (e.g. a
/// production-appropriate file path); `environments` lets a specific
/// environment (as named by `[targets.*].environment`) use something else,
/// such as a local file for `dev`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SecretDefinition {
    pub default: SecretSource,
    #[serde(default)]
    pub environments: HashMap<String, SecretSource>,
}

impl SecretDefinition {
    fn source_for<'a>(&'a self, environment: &str) -> &'a SecretSource {
        self.environments.get(environment).unwrap_or(&self.default)
    }
}

/// Whether `secret()` should return real values or a masked placeholder.
///
/// This only affects what's *returned* to the template — secrets are always
/// actually resolved, so `migration build`/`test build` still verify a
/// secret is reachable (and that a `literal` source is marked `insecure`)
/// even though they mask the result. It's decided once per render, not per
/// secret: Spawn renders a whole template in a single streaming pass to a
/// single destination, so there's no case where part of one render should be
/// masked and another part not. Migration applies reveal secrets because
/// they execute the rendered SQL for real. Build commands mask by default,
/// while `test run`/`compare`/`expect` always mask secrets to prevent them
/// from being printed or persisted in expected output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretsRenderMode {
    Masked,
    Revealed,
}

/// A secret's definition paired with its resolved value for the current
/// render. Bundling the two together means a resolved value is only ever
/// reachable through the definition that produced it, rather than living in
/// a second map that merely happens to share keys with the definitions.
struct Secret {
    definition: SecretDefinition,
    // Populated the first time this secret is resolved during a render, so a
    // secret referenced more than once resolves to the same value even if
    // its source (e.g. a command) isn't deterministic.
    resolved: Mutex<Option<String>>,
}

impl Secret {
    fn new(definition: SecretDefinition) -> Self {
        Self {
            definition,
            resolved: Mutex::new(None),
        }
    }

    async fn resolve(&self, environment: &str, operator: &Operator) -> Result<String> {
        if let Some(value) = self.resolved.lock().unwrap().as_ref() {
            return Ok(value.clone());
        }

        let source = self.definition.source_for(environment);
        let value = source.resolve(operator).await?;

        *self.resolved.lock().unwrap() = Some(value.clone());
        Ok(value)
    }
}

/// Resolves named secrets against the current environment and render mode.
pub struct SecretsRepository {
    secrets: HashMap<String, Secret>,
    environment: String,
    mode: SecretsRenderMode,
    operator: Operator,
}

impl SecretsRepository {
    pub fn new(
        definitions: HashMap<String, SecretDefinition>,
        environment: String,
        mode: SecretsRenderMode,
        operator: Operator,
    ) -> Self {
        let secrets = definitions
            .into_iter()
            .map(|(name, definition)| (name, Secret::new(definition)))
            .collect();
        Self {
            secrets,
            environment,
            mode,
            operator,
        }
    }

    /// A repository with no secrets defined, used where no `spawn.toml`
    /// secrets config is available (e.g. Spawn's own internal migrations).
    pub fn empty(operator: Operator) -> Self {
        Self::new(
            HashMap::new(),
            "prod".to_string(),
            SecretsRenderMode::Revealed,
            operator,
        )
    }

    /// The error for a `secret()` call naming something that isn't configured.
    ///
    /// Lists the defined names so a typo is obvious; names are safe to print,
    /// their values are not. Single line, because minijinja appends
    /// `(in <file>:<line>)` after it.
    fn undefined_secret(&self, name: &str) -> anyhow::Error {
        let mut known: Vec<&str> = self.secrets.keys().map(String::as_str).collect();
        known.sort_unstable();
        let defined = if known.is_empty() {
            "none are defined".to_string()
        } else {
            format!("defined: {}", known.join(", "))
        };

        anyhow!(
            "secret '{name}' is not defined in spawn.toml ({defined}); see {url}",
            url = crate::docs::SECRETS
        )
    }

    pub async fn resolve(&self, name: &str) -> Result<String> {
        let secret = self
            .secrets
            .get(name)
            .ok_or_else(|| self.undefined_secret(name))?;

        // Always resolve for real, even when masking the result: this is
        // what lets `build`/`test build` verify a secret is reachable
        // without ever displaying it.
        let value = secret
            .resolve(&self.environment, &self.operator)
            .await
            .with_context(|| format!("failed to resolve secret '{}'", name))?;

        Ok(match self.mode {
            SecretsRenderMode::Masked => format!("***MASKED:{}***", name),
            SecretsRenderMode::Revealed => value,
        })
    }

    /// Every secret value actually resolved so far during this render.
    /// Used to redact captured process output (e.g. psql stderr) that may
    /// otherwise echo a secret back verbatim — see the Secrets guide's note
    /// on why this is a render-mode-independent, best-effort measure.
    pub fn resolved_values(&self) -> Vec<String> {
        self.secrets
            .values()
            .filter_map(|secret| secret.resolved.lock().unwrap().clone())
            .collect()
    }
}

/// Replaces every value this render actually resolved with a placeholder,
/// across the error's full chain, and rebuilds a flat error from the
/// result. Running real SQL means a failing statement (e.g. a constraint
/// violation) can make the database echo a literal value — possibly a
/// secret — back in its own error output, entirely independent of the
/// checksum protection on what spawn persists. See the Secrets guide for
/// the residual risk this doesn't close.
///
/// Redacts both the raw resolved value and its SQL-escaped form (via the
/// engine's own dialect-specific escaper, so this stays correct as more
/// engines are added). Some errors echo back a parsed value (matching the
/// raw form), but others — e.g. a "LINE 1: ..." context on a syntax error —
/// echo the submitted SQL text verbatim, which shows the escaped literal
/// (quotes doubled, wrapped in quotes) rather than the raw string. A secret
/// containing a quote or other special character would otherwise leak
/// through that path untouched.
pub fn redact_secrets(
    err: anyhow::Error,
    secrets: &SecretsRepository,
    dialect: crate::sql_formatter::SqlDialect,
) -> anyhow::Error {
    anyhow!(redact_text(&format!("{:?}", err), secrets, dialect))
}

/// [`redact_secrets`] over plain text, for output that is shown rather than
/// returned — `script run` collects psql's diagnostics and passes the tail
/// through this before printing it.
///
/// Safe to call only once the run is over: it can replace a value only if the
/// render has already resolved it.
pub fn redact_text(
    text: &str,
    secrets: &SecretsRepository,
    dialect: crate::sql_formatter::SqlDialect,
) -> String {
    let mut text = text.to_string();
    let mut values = secrets.resolved_values();
    let escaped: Vec<String> = values
        .iter()
        .map(|value| crate::sql_formatter::escape_string(dialect, value))
        .collect();
    values.extend(escaped);
    // Longest first: if a shorter value (raw or escaped) is a substring of a
    // longer one, redacting the shorter one first would consume part of the
    // longer one and leave the rest of it exposed.
    values.sort_by_key(|v| std::cmp::Reverse(v.len()));
    for value in values {
        if value.is_empty() {
            continue;
        }
        text = text.replace(&value, "***REDACTED***");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use opendal::services::Memory;

    fn memory_operator() -> Operator {
        Operator::new(Memory::default()).unwrap()
    }

    fn repo(
        definitions: HashMap<String, SecretDefinition>,
        environment: &str,
        mode: SecretsRenderMode,
    ) -> SecretsRepository {
        SecretsRepository::new(
            definitions,
            environment.to_string(),
            mode,
            memory_operator(),
        )
    }

    fn defs(entries: Vec<(&str, SecretDefinition)>) -> HashMap<String, SecretDefinition> {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    #[tokio::test]
    async fn resolves_env_source() {
        std::env::set_var("SPAWN_TEST_SECRET_ENV", "sekrit");
        let definitions = defs(vec![(
            "application_password",
            SecretDefinition {
                default: SecretSource::Env {
                    name: "SPAWN_TEST_SECRET_ENV".to_string(),
                },
                environments: HashMap::new(),
            },
        )]);
        let repository = repo(definitions, "prod", SecretsRenderMode::Revealed);
        let value = repository.resolve("application_password").await.unwrap();
        assert_eq!(value, "sekrit");
        std::env::remove_var("SPAWN_TEST_SECRET_ENV");
    }

    #[tokio::test]
    async fn resolves_file_source_and_strips_trailing_newline() {
        let op = memory_operator();
        op.write("secret.txt", "file-secret\n").await.unwrap();
        let definitions = defs(vec![(
            "application_password",
            SecretDefinition {
                default: SecretSource::File {
                    path: "secret.txt".to_string(),
                },
                environments: HashMap::new(),
            },
        )]);
        let repository = SecretsRepository::new(
            definitions,
            "prod".to_string(),
            SecretsRenderMode::Revealed,
            op,
        );
        let value = repository.resolve("application_password").await.unwrap();
        assert_eq!(value, "file-secret");
    }

    #[tokio::test]
    async fn environment_override_takes_precedence_over_default() {
        let op = memory_operator();
        op.write("dev-secret.txt", "dev-value").await.unwrap();
        op.write("prod-secret.txt", "prod-value").await.unwrap();
        let definitions = defs(vec![(
            "application_password",
            SecretDefinition {
                default: SecretSource::File {
                    path: "prod-secret.txt".to_string(),
                },
                environments: HashMap::from([(
                    "dev".to_string(),
                    SecretSource::File {
                        path: "dev-secret.txt".to_string(),
                    },
                )]),
            },
        )]);
        let repository = SecretsRepository::new(
            definitions.clone(),
            "dev".to_string(),
            SecretsRenderMode::Revealed,
            op.clone(),
        );
        assert_eq!(
            repository.resolve("application_password").await.unwrap(),
            "dev-value"
        );

        let repository = SecretsRepository::new(
            definitions,
            "prod".to_string(),
            SecretsRenderMode::Revealed,
            op,
        );
        assert_eq!(
            repository.resolve("application_password").await.unwrap(),
            "prod-value"
        );
    }

    #[tokio::test]
    async fn masked_mode_hides_a_resolvable_secret() {
        std::env::set_var("SPAWN_TEST_SECRET_MASKED", "sekrit");
        let definitions = defs(vec![(
            "application_password",
            SecretDefinition {
                default: SecretSource::Env {
                    name: "SPAWN_TEST_SECRET_MASKED".to_string(),
                },
                environments: HashMap::new(),
            },
        )]);
        let repository = repo(definitions, "prod", SecretsRenderMode::Masked);
        let value = repository.resolve("application_password").await.unwrap();
        assert_eq!(value, "***MASKED:application_password***");
        std::env::remove_var("SPAWN_TEST_SECRET_MASKED");
    }

    #[tokio::test]
    async fn masked_mode_still_verifies_the_secret_is_reachable() {
        // Masking must not skip resolution: this is what lets `build`/`test
        // build` catch a missing env var (or an insecure literal) without
        // ever displaying the real value.
        let definitions = defs(vec![(
            "application_password",
            SecretDefinition {
                default: SecretSource::Env {
                    name: "SPAWN_TEST_SECRET_MISSING".to_string(),
                },
                environments: HashMap::new(),
            },
        )]);
        let repository = repo(definitions, "prod", SecretsRenderMode::Masked);
        let err = repository
            .resolve("application_password")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("failed to resolve secret"));
    }

    #[tokio::test]
    async fn masked_mode_still_rejects_insecure_literal() {
        let definitions = defs(vec![(
            "application_password",
            SecretDefinition {
                default: SecretSource::Literal {
                    value: "hunter2".to_string(),
                    insecure: false,
                },
                environments: HashMap::new(),
            },
        )]);
        let repository = repo(definitions, "prod", SecretsRenderMode::Masked);
        let err = repository
            .resolve("application_password")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("failed to resolve secret"));
    }

    #[tokio::test]
    async fn missing_secret_with_none_configured_says_so_and_shows_the_fix() {
        let repository = repo(HashMap::new(), "prod", SecretsRenderMode::Revealed);
        let err = repository.resolve("nope").await.unwrap_err();
        let message = err.to_string();
        assert!(message.contains("secret 'nope' is not defined in spawn.toml"));
        assert!(
            message.contains("none are defined"),
            "should say nothing is configured, got: {message}"
        );
        assert!(
            message.contains("https://docs.spawn.dev/guides/secrets/"),
            "should point at the secrets guide, got: {message}"
        );
    }

    #[tokio::test]
    async fn missing_secret_lists_the_defined_names_but_never_their_values() {
        let definitions = defs(vec![
            (
                "zeta_password",
                SecretDefinition {
                    default: SecretSource::Literal {
                        value: "s3cret-zeta-value".to_string(),
                        insecure: true,
                    },
                    environments: HashMap::new(),
                },
            ),
            (
                "alpha_password",
                SecretDefinition {
                    default: SecretSource::Literal {
                        value: "s3cret-alpha-value".to_string(),
                        insecure: true,
                    },
                    environments: HashMap::new(),
                },
            ),
        ]);
        let repository = repo(definitions, "prod", SecretsRenderMode::Revealed);
        let err = repository.resolve("aplha_password").await.unwrap_err();
        let message = format!("{:?}", err);

        assert!(message.contains("secret 'aplha_password' is not defined in spawn.toml"));
        assert!(
            message.contains("defined: alpha_password, zeta_password"),
            "should list defined names in sorted order, got: {message}"
        );
        // Names are safe to print; their values are not.
        assert!(
            !message.contains("s3cret-alpha-value") && !message.contains("s3cret-zeta-value"),
            "a configured secret's value must never appear, got: {message}"
        );
    }

    #[tokio::test]
    async fn literal_source_requires_insecure_flag() {
        let definitions = defs(vec![(
            "application_password",
            SecretDefinition {
                default: SecretSource::Literal {
                    value: "hunter2".to_string(),
                    insecure: false,
                },
                environments: HashMap::new(),
            },
        )]);
        let repository = repo(definitions, "prod", SecretsRenderMode::Revealed);
        let err = repository
            .resolve("application_password")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("failed to resolve secret"));
    }

    #[tokio::test]
    async fn literal_source_with_insecure_flag_resolves() {
        let definitions = defs(vec![(
            "application_password",
            SecretDefinition {
                default: SecretSource::Literal {
                    value: "hunter2".to_string(),
                    insecure: true,
                },
                environments: HashMap::new(),
            },
        )]);
        let repository = repo(definitions, "prod", SecretsRenderMode::Revealed);
        assert_eq!(
            repository.resolve("application_password").await.unwrap(),
            "hunter2"
        );
    }

    #[tokio::test]
    async fn resolve_caches_command_source_across_calls() {
        // Each invocation of `sh -c 'echo $$'` reports a different PID, so
        // if resolve() weren't caching, two calls would return different
        // values.
        let definitions = defs(vec![(
            "application_password",
            SecretDefinition {
                default: SecretSource::Command {
                    command: vec!["sh".to_string(), "-c".to_string(), "echo $$".to_string()],
                },
                environments: HashMap::new(),
            },
        )]);
        let repository = repo(definitions, "prod", SecretsRenderMode::Revealed);
        let first = repository.resolve("application_password").await.unwrap();
        let second = repository.resolve("application_password").await.unwrap();
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn host_file_source_reads_absolute_path_regardless_of_operator_root() {
        // Operator rooted somewhere unrelated, to prove host_file bypasses it.
        let secret_dir = tempfile::tempdir().unwrap();
        let secret_path = secret_dir.path().join("application-password");
        std::fs::write(&secret_path, "host-secret\n").unwrap();

        let definitions = defs(vec![(
            "application_password",
            SecretDefinition {
                default: SecretSource::HostFile {
                    path: secret_path.to_str().unwrap().to_string(),
                },
                environments: HashMap::new(),
            },
        )]);
        let repository = repo(definitions, "prod", SecretsRenderMode::Revealed);
        let value = repository.resolve("application_password").await.unwrap();
        assert_eq!(value, "host-secret");
    }

    #[tokio::test]
    async fn file_source_cannot_reach_an_absolute_host_path() {
        // opendal resolves paths relative to the operator's root even when
        // they look absolute, so this must fail rather than silently
        // resolve somewhere under the root.
        let unrelated_root = tempfile::tempdir().unwrap();
        let op = Operator::new(
            opendal::services::Fs::default().root(unrelated_root.path().to_str().unwrap()),
        )
        .unwrap();

        let secret_dir = tempfile::tempdir().unwrap();
        let secret_path = secret_dir.path().join("application-password");
        std::fs::write(&secret_path, "host-secret").unwrap();

        let definitions = defs(vec![(
            "application_password",
            SecretDefinition {
                default: SecretSource::File {
                    path: secret_path.to_str().unwrap().to_string(),
                },
                environments: HashMap::new(),
            },
        )]);
        let repository = SecretsRepository::new(
            definitions,
            "prod".to_string(),
            SecretsRenderMode::Revealed,
            op,
        );
        let err = repository
            .resolve("application_password")
            .await
            .unwrap_err();
        // .to_string() only prints the outer context; the underlying "could
        // not read secret file" is deeper in the anyhow chain.
        assert!(format!("{:?}", err).contains("could not read secret file"));
    }

    #[tokio::test]
    async fn redact_secrets_strips_resolved_values_from_the_full_error_chain() {
        let mut definitions = HashMap::new();
        definitions.insert(
            "application_password".to_string(),
            SecretDefinition {
                default: SecretSource::Literal {
                    value: "hunter2".to_string(),
                    insecure: true,
                },
                environments: HashMap::new(),
            },
        );
        let op = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
        let secrets = SecretsRepository::new(
            definitions,
            "prod".to_string(),
            SecretsRenderMode::Revealed,
            op,
        );
        // Resolve it, as a real render would while streaming to psql.
        secrets.resolve("application_password").await.unwrap();

        // Value at the deepest level. Only reached because redact_secrets
        // formats with `{:?}`, which walks `source()`; `{}` would leak it.
        let simulated_psql_error = anyhow!(
            "psql exited with code 1: ERROR: duplicate key value\nDETAIL: Key (password)=(hunter2) already exists."
        )
        .context("could not render include: error in \"roles/app_role.sql\"")
        .context("Migration '20260101000000-bootstrap' failed");

        let redacted = redact_secrets(
            simulated_psql_error,
            &secrets,
            crate::sql_formatter::SqlDialect::Postgres,
        );

        let full_text = format!("{:?}", redacted);
        assert!(!full_text.contains("hunter2"));
        assert!(full_text.contains("***REDACTED***"));
    }

    #[tokio::test]
    async fn redact_secrets_fully_redacts_a_secret_that_contains_another_as_a_substring() {
        // This test will only fail some of the time if the sort order is ever
        // changed to be random, but that's better than nothing.
        let mut definitions = HashMap::new();
        definitions.insert(
            "short".to_string(),
            SecretDefinition {
                default: SecretSource::Literal {
                    value: "hunter2".to_string(),
                    insecure: true,
                },
                environments: HashMap::new(),
            },
        );
        definitions.insert(
            "long".to_string(),
            SecretDefinition {
                default: SecretSource::Literal {
                    value: "hunter2suffix".to_string(),
                    insecure: true,
                },
                environments: HashMap::new(),
            },
        );
        let op = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
        let secrets = SecretsRepository::new(
            definitions,
            "prod".to_string(),
            SecretsRenderMode::Revealed,
            op,
        );
        secrets.resolve("short").await.unwrap();
        secrets.resolve("long").await.unwrap();

        let simulated_psql_error = anyhow!(
            "psql exited with code 1: ERROR: duplicate key value\nDETAIL: Key (password)=(hunter2suffix) already exists."
        );

        let redacted = redact_secrets(
            simulated_psql_error,
            &secrets,
            crate::sql_formatter::SqlDialect::Postgres,
        );

        // Regardless of which order resolved_values() happens to hand back
        // the two secrets, the longer one must be redacted whole — not just
        // the "hunter2" prefix it shares with the shorter secret.
        let full_text = format!("{:?}", redacted);
        assert!(!full_text.contains("hunter2"), "got: {}", full_text);
        assert!(!full_text.contains("suffix"), "got: {}", full_text);
        assert!(full_text.contains("***REDACTED***"), "got: {}", full_text);
    }

    #[tokio::test]
    async fn redact_secrets_also_strips_the_escaped_form_of_a_secret() {
        // A secret containing a quote renders into the SQL sent to psql as
        // an escaped literal (quotes doubled, wrapped in quotes). Some
        // Postgres errors echo back the submitted SQL text verbatim (e.g. a
        // "LINE 1: ..." context on a syntax error) rather than the parsed
        // value, so that escaped form — not just the raw value — must be
        // redacted too.
        let mut definitions = HashMap::new();
        definitions.insert(
            "application_password".to_string(),
            SecretDefinition {
                default: SecretSource::Literal {
                    value: "hunter'2".to_string(),
                    insecure: true,
                },
                environments: HashMap::new(),
            },
        );
        let op = opendal::Operator::new(opendal::services::Memory::default()).unwrap();
        let secrets = SecretsRepository::new(
            definitions,
            "prod".to_string(),
            SecretsRenderMode::Revealed,
            op,
        );
        secrets.resolve("application_password").await.unwrap();

        // Simulates a "LINE 1: ..." context echoing the submitted SQL text,
        // which shows the escaped literal 'hunter''2' rather than the raw
        // value hunter'2.
        let simulated_psql_error = anyhow!(
            "psql exited with code 1: ERROR: syntax error at or near \"FRIM\"\n\
             LINE 1: SELECT * FRIM foo WHERE password = 'hunter''2';"
        );

        let redacted = redact_secrets(
            simulated_psql_error,
            &secrets,
            crate::sql_formatter::SqlDialect::Postgres,
        );

        let full_text = format!("{:?}", redacted);
        assert!(!full_text.contains("hunter"), "got: {}", full_text);
        assert!(full_text.contains("***REDACTED***"), "got: {}", full_text);
    }

    /// `script run` shows psql's diagnostics rather than returning them, so it
    /// redacts the text directly. Both forms have to go, for the same reasons
    /// as the error path: a `DETAIL` line echoes the parsed value, a `LINE 1:`
    /// context echoes the escaped literal.
    #[tokio::test]
    async fn redact_text_strips_both_forms_from_collected_diagnostics() {
        let definitions = defs(vec![(
            "application_password",
            SecretDefinition {
                default: SecretSource::Literal {
                    value: "hunter'2".to_string(),
                    insecure: true,
                },
                environments: HashMap::new(),
            },
        )]);
        let secrets = repo(definitions, "prod", SecretsRenderMode::Revealed);
        secrets.resolve("application_password").await.unwrap();

        let diagnostics = "ERROR:  duplicate key value violates unique constraint\n\
                           DETAIL:  Key (password)=(hunter'2) already exists.\n\
                           LINE 1: UPDATE u SET password = 'hunter''2';";

        let redacted = redact_text(
            diagnostics,
            &secrets,
            crate::sql_formatter::SqlDialect::Postgres,
        );

        assert!(!redacted.contains("hunter"), "got: {}", redacted);
        assert!(redacted.contains("***REDACTED***"), "got: {}", redacted);
        // The surrounding diagnostics are still readable.
        assert!(redacted.contains("duplicate key value"), "got: {}", redacted);
    }
}
