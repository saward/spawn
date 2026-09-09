---
title: Configuration File (spawn.toml)
description: Complete reference for the Spawn configuration file.
---

The `spawn.toml` file configures your Spawn project, defining database connections and project structure.

## File location

By default, Spawn looks for `spawn.toml` in the current directory. Override with `--config-file`:

```bash
spawn --config-file /path/to/config.toml migration apply
```

## Top-level fields

### `spawn_folder`

**Type:** String  
**Required:** Yes

Path to the directory containing migrations, components, tests, and pinned snapshots.

```toml
spawn_folder = "./database/spawn"
```

This would expect the following directory layout:

- `./database/spawn/migrations/`
- `./database/spawn/components/`
- `./database/spawn/tests/`
- `./database/spawn/pinned/`

### `template_up`

**Type:** String  
**Required:** No  
**Default:** None (uses the built-in default template)

Path to a custom template file used by `spawn migration new` instead of the built-in default. Resolved relative to `spawn_folder`.

```toml
template_up = "templates/custom-up.sql"
```

With this set, `spawn migration new` copies the contents of `spawn_folder/templates/custom-up.sql` into the new migration's `up.sql` instead of the built-in default:

```sql
BEGIN;

COMMIT;
```

### `template_test`

**Type:** String  
**Required:** No  
**Default:** None (uses the built-in default template)

Path to a custom template file used by `spawn test new` instead of the built-in default. Resolved relative to `spawn_folder`.

```toml
template_test = "templates/custom-test.sql"
```

With this set, `spawn test new` copies the contents of `spawn_folder/templates/custom-test.sql` into the new test's `test.sql` instead of the built-in default:

```sql
-- Test file
SELECT 1;
```

### `target`

**Type:** String  
**Required:** No  
**Default:** None

The default target to use for commands. Must match a key in `[targets]`.

```toml
target = "local"
```

Override per-command with `--target`:

```bash
spawn --target production migration status
```

### `environment`

**Type:** String  
**Required:** No  
**Default:** None

Global environment override. Overrides the `environment` field in target configs.

```toml
environment = "dev"
```

This is rarely set at the top level. Usually each target defines its own environment.

### `project_id`

**Type:** String (UUID)  
**Required:** No  
**Default:** Auto-generated on `spawn init`

Unique and anonymous identifier for telemetry. Generated automatically by `spawn init`.

```toml
project_id = "a1b2c3d4-e5f6-7890-abcd-ef1234567890"
```

### `telemetry`

**Type:** Boolean  
**Required:** No  
**Default:** `true`

Whether to send anonymous usage telemetry.

```toml
telemetry = false
```

Set the `DO_NOT_TRACK` environment variable to disable telemetry globally.

### `secrets`

**Type:** Table  
**Required:** No

Named secrets available to templates via the [`secret()`](/reference/templating/#secret) function. See [Secrets](#secrets) below for the field format, and the [Secrets guide](/guides/secrets/) for how they're used in templates and which commands reveal vs. mask them.

## Target configurations

The `[targets]` section defines one or more database connections. Each target is a table with the following fields. For practical setup examples including Docker and Google Cloud SQL, see the [Database Connections guide](/guides/manage-databases/).

### `engine`

**Type:** String  
**Required:** Yes  
**Values:** `"postgres-psql"`

The database engine type. Currently only PostgreSQL via psql is supported.

```toml
[targets.local]
engine = "postgres-psql"
```

### `spawn_database`

**Type:** String  
**Required:** No

The database name where Spawn stores migration tracking tables (in the `spawn_schema`). If not provided, defaults to using the same database that your connection command uses. This database must already exist.

```toml
spawn_database = "spawn"
```

### `spawn_schema`

**Type:** String  
**Required:** No  
**Default:** `"_spawn"`

The schema where Spawn creates its internal tracking tables (`migration_history`, etc.). This schema will be created if it does not yet exist.

```toml
spawn_schema = "_spawn"
```

### `environment`

**Type:** String  
**Required:** No  
**Default:** `"prod"`

Environment identifier. Available in migration [templates](/reference/templating) as `{{ env }}`. Used for conditional logic:

```sql
{% if env == "dev" %}
INSERT INTO test_data VALUES ('sample');
{% endif %}
```

Common values: `"dev"`, `"staging"`, `"prod"`

```toml
environment = "dev"
```

### `command`

**Type:** Table (CommandSpec)  
**Required:** Yes

Specifies how to execute SQL against the target. Two modes: `direct` and `provider`. For now, only connection via PostgreSQL psql is supported, so this should be the command that allows piping changes to the database. See the [Database Connections guide](/guides/manage-databases/#command-configuration) for detailed examples of both modes.

#### Direct command

Use when you have a straightforward way to invoke psql.

```toml
command = { kind = "direct", direct = ["psql", "-U", "postgres", "mydb"] }
```

**Docker example:**

```toml
command = { kind = "direct", direct = [
  "docker", "exec", "-i", "mydb-container",
  "psql", "-U", "postgres", "mydb"
] }
```

#### Provider command

Use when the connection details need to be resolved dynamically, or are faster to resolve once per command for faster performance (e.g., via `gcloud`).

The `provider` command must output the command as a shell command.

Spawn runs the `provider` command, parses the command, then executes the resolved command with `append` args added.

**Google Cloud SQL example:**

```toml
command = {
  kind = "provider",
  provider = [
    "gcloud", "compute", "ssh", "db-instance",
    "--zone", "us-central1-a",
    "--project", "my-project",
    "--dry-run"
  ],
  append = ["-T", "sudo", "-u", "postgres", "psql", "mydb"]
}
```

The `--dry-run` flag makes `gcloud` output the SSH command as a string instead of executing it.

## Secrets

The `[secrets]` table defines values that templates can read via the [`secret()`](/reference/templating/#secret) function, so passwords and other sensitive values don't need to be committed as plain template variables. See the [Secrets guide](/guides/secrets/) for how secrets are used in templates and which commands reveal vs. mask them; this section covers the `spawn.toml` field format.

Each secret has a `default` source and optional per-environment overrides, keyed by the same environment names used by `[targets.*].environment`.

```toml
[secrets.application_password.default]
source = "host_file"
path = "/run/secrets/application-password"

[secrets.application_password.environments.dev]
source = "file"
path = "./local-secrets/application-password.txt"
```

When a template calls `secret("application_password")`, Spawn looks up `environments.<current environment>` first, falling back to `default` if there's no override for the current environment. `default` is required — there's no implicit "no secret configured" fallback.

### Sources

#### `env`

Reads an OS environment variable.

```toml
[secrets.application_password.default]
source = "env"
name = "APPLICATION_PASSWORD"
```

#### `file`

Reads a file via spawn's configured operator, with any trailing newline stripped — resolved the same way any other file spawn reads is (`read_file`, migration/component lookups, etc.). It is **not** a real filesystem path, so an absolute path here will not reach a real host location — use `host_file` for that.

```toml
[secrets.application_password.environments.dev]
source = "file"
path = "./local-secrets/application-password.txt"
```

#### `host_file`

Reads a file directly from the host filesystem, bypassing the operator, with any trailing newline stripped. Use this for secrets mounted on the host outside spawn's storage — Docker/Kubernetes secrets under `/run/secrets`, systemd's `LoadCredential=`, or an already-decrypted `sops`/`gpg` output file.

```toml
[secrets.application_password.default]
source = "host_file"
path = "/run/secrets/application-password"
```

#### `command`

Runs a command and uses its trimmed stdout as the value. Useful for secret managers with a CLI (Vault, 1Password, `sops`, `systemd-creds`, etc.).

```toml
[secrets.application_password.default]
source = "command"
command = ["op", "read", "op://vault/application-password/password"]
```

#### `literal`

An inline value. Requires `insecure = true` — Spawn refuses to use a `literal` secret without it, so a plaintext value committed to `spawn.toml` can't accidentally become a project's "secure default". This is enforced whenever the secret is resolved (including a masked `migration build`), not just when it's actually displayed. Intended for local development only.

```toml
[secrets.application_password.environments.dev]
source = "literal"
value = "dev-only-password"
insecure = true
```

See [Secrets: Masking](/guides/secrets/#masking) for which commands reveal real values vs. mask them.

## Complete example

```toml
spawn_folder = "./database/spawn"
target = "local"
project_id = <replace with random uuid>

[targets.local]
spawn_database = "spawn"
spawn_schema = "_spawn"
environment = "dev"
engine = "postgres-psql"
command = {
  kind = "direct",
  direct = ["docker", "exec", "-i", "mydb", "psql", "-U", "postgres", "postgres"]
}

[targets.staging]
spawn_database = "spawn"
spawn_schema = "_spawn"
engine = "postgres-psql"
environment = "prod"
command = {
  kind = "provider",
  provider = [
    "gcloud", "compute", "ssh", "staging-db",
    "--zone", "us-central1-a",
    "--project", "my-project-staging",
    "--dry-run"
  ],
  append = ["-T", "sudo", "-u", "postgres", "psql", "mydb"]
}

[targets.production]
spawn_database = "spawn"
spawn_schema = "_spawn"
engine = "postgres-psql"
environment = "prod"
command = {
  kind = "provider",
  provider = [
    "gcloud", "compute", "ssh", "prod-db",
    "--zone", "us-east1-b",
    "--project", "my-project-prod",
    "--dry-run"
  ],
  append = ["-T", "sudo", "-u", "postgres", "psql", "mydb"]
}
```

## Environment variable overrides

Spawn supports environment variable overrides with the `SPAWN_` prefix:

```bash
export SPAWN_TARGET=production
spawn migration status
```

This is equivalent to `spawn --target production migration status`.
