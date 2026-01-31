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

### `database`

**Type:** String  
**Required:** No  
**Default:** None

The default database to use for commands. Must match a key in `[databases]`.

```toml
database = "local"
```

Override per-command with `--database`:

```bash
spawn --database production migration status
```

### `environment`

**Type:** String  
**Required:** No  
**Default:** None

Global environment override. Overrides the `environment` field in database configs.

```toml
environment = "dev"
```

This is rarely set at the top level. Usually each database defines its own environment.

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

## Database configurations

The `[databases]` section defines one or more database connections. Each database is a table with the following fields. For practical setup examples including Docker and Google Cloud SQL, see the [Database Connections guide](/guides/manage-databases/).

### `engine`

**Type:** String  
**Required:** Yes  
**Values:** `"postgres-psql"`

The database engine type. Currently only PostgreSQL via psql is supported.

```toml
[databases.local]
engine = "postgres-psql"
```

### `spawn_database`

**Type:** String  
**Required:** Yes

The database name where Spawn stores migration tracking tables (in the `spawn_schema`).

```toml
spawn_database = "spawn"
```

### `spawn_schema`

**Type:** String  
**Required:** No  
**Default:** `"_spawn"`

The schema where Spawn creates its internal tracking tables (`migration_history`, etc.).

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

Specifies how to execute SQL against the database. Two modes: `direct` and `provider`. For now, only connection via PostgreSQL psql is supported, so this should be the command that allows piping changes to the database. See the [Database Connections guide](/guides/manage-databases/#command-configuration) for detailed examples of both modes.

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

## Complete example

```toml
spawn_folder = "./database/spawn"
database = "local"
project_id = <replace with random uuid>

[databases.local]
spawn_database = "spawn"
spawn_schema = "_spawn"
environment = "dev"
engine = "postgres-psql"
command = {
  kind = "direct",
  direct = ["docker", "exec", "-i", "mydb", "psql", "-U", "postgres", "postgres"]
}

[databases.staging]
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

[databases.production]
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
export SPAWN_DATABASE=production
spawn migration status
```

This is equivalent to `spawn --database production migration status`.
