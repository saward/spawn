---
title: Managing Secrets
description: Use passwords and other secrets in Spawn templates.
---

Spawn migrations sometimes need real secrets — a password for a role being created, an API key seeded into a config table, and so on. The `[secrets]` table in `spawn.toml` declares where each secret comes from, and the `secret()` template function reads it at render time, so real values never need to be committed to your repo as plain [variables](/reference/templating/#variables).

## Defining secrets

Each secret has a `default` source, and optional per-environment overrides keyed by the same environment names used by `[targets.*].environment`:

```toml
[secrets.application_password.default]
source = "file"
path = "/run/secrets/application-password"

[secrets.application_password.environments.dev]
source = "file"
path = "./local-secrets/application-password.txt"
```

When a template calls `secret("application_password")`, Spawn looks up `environments.<current environment>` first, falling back to `default` if there's no override for the current environment. `default` is required — there's no implicit "no secret configured" fallback, so pick a `default` that's safe for production and override it for looser environments like `dev`.

See the [configuration reference](/reference/config/#secrets) for the exact fields of every `source` type.

## Choosing a source

There are four ways to tell Spawn where a secret's value actually lives:

- **`env`** reads an OS environment variable — the natural fit when a secret is already injected that way, e.g. by a CI/CD pipeline or a container orchestrator's own secret injection.
- **`file`** reads a file's contents. Covers Docker/Kubernetes secrets mounted under `/run/secrets`, systemd's `LoadCredential=`, or an already-decrypted `sops`/`gpg` output file.
- **`command`** runs a command and uses its trimmed stdout as the value.
- **`literal`** is an inline value in `spawn.toml`, gated behind an explicit `insecure = true` flag.

`command` is a general-purpose escape hatch, and most secret managers ship a CLI that can print a value to stdout, so Vault, 1Password, AWS Secrets Manager, GCP Secret Manager, Azure Key Vault, Doppler, Bitwarden, `pass`, and `systemd-creds` are all reachable this way without Spawn needing a dedicated integration for each one:

```toml
[secrets.application_password.default]
source = "command"
command = ["op", "read", "op://vault/application-password/password"]
```

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

## Secrets and pinning

Secrets are entirely outside of [pinning](/cli/migration-pin/): `spawn migration pin` snapshots component _content_ into the content-addressed store and records it in a migration's `lock.toml`. It never touches `[secrets]` or resolved secret values — a secret's source is declared once in `spawn.toml`, and its value is re-resolved fresh every time a migration is built or applied, regardless of pinning.
