---
title: Managing Secrets
description: Use passwords and other secrets in Spawn templates.
---

Spawn migrations sometimes need real secrets — a password for a role being created, an API key seeded into a config table, and so on. The `[secrets]` table in `spawn.toml` declares where each secret comes from, and the `secret()` template function reads it at render time, so real values never need to be committed to your repo as plain [variables](/reference/templating/#variables).

## Defining secrets

Each secret has a `default` source, and optional per-environment overrides keyed by the same environment names used by `[targets.*].environment`:

```toml
[secrets.application_password.default]
source = "host_file"
path = "/run/secrets/application-password"

[secrets.application_password.environments.dev]
source = "file"
path = "./local-secrets/application-password.txt"
```

When a template calls `secret("application_password")`, Spawn looks up `environments.<current environment>` first, falling back to `default` if there's no override for the current environment. `default` is required — there's no implicit "no secret configured" fallback, so pick a `default` that's safe for production and override it for looser environments like `dev`.

See the [configuration reference](/reference/config/#secrets) for the exact fields of every `source` type.

## Choosing a source

There are five ways to tell Spawn where a secret's value actually lives:

- **`env`** reads an OS environment variable — the natural fit when a secret is already injected that way, e.g. by a CI/CD pipeline or a container orchestrator's own secret injection.
- **`file`** reads a file via spawn's configured operator, resolved the same way any other file spawn reads is. For now, this is restricted to the project's path.
- **`host_file`** reads a file directly from the host filesystem, bypassing the operator. Use this for secrets mounted on the host outside spawn's storage — Docker/Kubernetes secrets under `/run/secrets`, systemd's `LoadCredential=`, or an already-decrypted `sops`/`gpg` output file. This is the one to use whenever the path is a real absolute host path.
- **`command`** runs a command and uses its trimmed stdout as the value.
- **`literal`** is an inline value in `spawn.toml`, gated behind an explicit `insecure = true` flag.

`command` is a general-purpose escape hatch, and most secret managers ship a CLI that can print a value to stdout, so Vault, 1Password, AWS Secrets Manager, GCP Secret Manager, Azure Key Vault, Doppler, Bitwarden, `pass`, and `systemd-creds` are all reachable this way without Spawn needing a dedicated integration for each one:

```toml
[secrets.application_password.default]
source = "command"
command = ["op", "read", "op://vault/application-password/password"]
```

If a `command` source fails, Spawn never learns its value — so unlike every other secret-disclosure path, that value can't be found and redacted afterwards. To guard against a provider script that logs its own inputs on failure (e.g. `echo "got: $PASSWORD" >&2; exit 1`), a failed command's stderr is not shown by default — only its exit code. Set `SPAWN_DEBUG_COMMAND_STDERR=1` to see the real stderr for local debugging. Never set this in CI or anywhere output may be logged or shared, since that's exactly what the default behavior protects against.

`literal` is not intended to be used for production and therefore has an `insecure` flag to ensure the user understands that this is not for production use. Spawn refuses to use a `literal` secret without it, so a plaintext value committed to `spawn.toml` can't accidentally become a project's "secure default." Reach for it only as a `dev`/local override, never as a `default`:

```toml
[secrets.application_password.environments.dev]
source = "literal"
value = "dev-only-password"
insecure = true
```

## Using `secret()` in templates

```sql
CREATE ROLE app_user WITH LOGIN PASSWORD {{ secret("application_password") }};
```

An unresolvable secret (missing environment variable, missing file, failing command, a `literal` without `insecure = true`, or a name not defined in `spawn.toml`) fails the render rather than silently producing an empty value. A secret referenced more than once in the same render resolves to the same value, even if its source (e.g. a `command`) isn't deterministic.

## Masking

Whether `secret()` returns the real value or a placeholder like `***MASKED:application_password***` depends on the command being run, not on anything in your template or config:

| Command                                                                                                | Secrets                                                                            |
| ------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------- |
| [`migration apply`](/cli/migration-apply/)                                                             | Always revealed — it executes the rendered SQL for real.                           |
| [`migration build`](/cli/migration-build/)                                                             | Masked by default. Pass `--reveal-secrets` to see real values for local debugging. |
| [`test run`](/cli/test-run/), [`test compare`](/cli/test-compare/), [`test expect`](/cli/test-expect/) | Always revealed — they execute the rendered SQL against a real database.           |
| [`test build`](/cli/test-build/)                                                                       | Masked by default. Pass `--reveal-secrets` to see real values for local debugging. |

Masking only affects the value _returned to the template_ — the secret is still fully resolved either way, so a masked `build` still fails if the secret is unreachable or misconfigured (including the `literal`/`insecure` check above). It just never displays the real value. This means `build` doubles as a way to verify your secrets are reachable in a given environment before you ever run `apply`.

## Secrets in failed applies

`migration apply` executes real SQL, and a failing statement can make the database echo a literal value back in its own error message — a unique or check constraint violation reporting the row's actual contents, an `invalid input syntax` error quoting the offending value, and so on. If that statement used `secret()`, the real value could otherwise end up in whatever captures `apply`'s output — a terminal, a CI log, anything.

To prevent this, Spawn replaces every secret value a failed `apply` actually resolved with `***REDACTED***` in the returned error, before it's ever displayed or logged.

:::caution[This is a best-effort substitution, not a guarantee]
The redaction works by matching the exact resolved value against the error text — it can't distinguish a secret's value from unrelated text that happens to be identical. That has one real consequence worth knowing: someone who can both **author** migrations and **apply** them against a target database could, in principle, deliberately craft a migration to test whether a guessed string matches a currently configured secret — by embedding the guess somewhere designed to fail, applying it, and checking whether the guess comes back redacted.

This is meaningfully harder to exploit than reading a log: it requires apply access to the actual database, not just read access to `_spawn.migration_history`, and each guess is a real, visible, failed apply rather than a silent offline check. But it means this mechanism is a defense against **accidental** disclosure (an ordinary failing migration leaking a value into CI logs), not a substitute for controlling who can author and apply migrations against sensitive targets.
:::

## Secrets and pinning

Secrets are entirely outside of [pinning](/cli/migration-pin/): `spawn migration pin` snapshots component _content_ into the content-addressed store and records it in a migration's `lock.toml`. It never touches `[secrets]` or resolved secret values — a secret's source is declared once in `spawn.toml`, and its value is re-resolved fresh every time a migration is built or applied, regardless of pinning.
