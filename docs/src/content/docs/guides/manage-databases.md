---
title: Database Connections
description: How to configure database connections in spawn.toml
---

Spawn requires a database connection to apply migrations and run tests. Database connections are configured in your `spawn.toml` file under the `[databases]` section. See the [configuration reference](/reference/config/#database-configurations) for the full list of database fields.

## Basic Configuration

Each database configuration requires:

- `engine`: The database engine type (currently only `"postgres-psql"`)
- `spawn_database`: The database name to connect to
- `spawn_schema`: The schema where spawn stores migration tracking (default: `"_spawn"`)
- `environment`: Environment name (e.g., `"dev"`, `"prod"`)
- `command`: How to execute SQL commands (see below)

## Command Configuration

The `command` field specifies how spawn should execute SQL against your database. There are two modes:

### 1. Direct Commands

Use a direct command when you have a straightforward way to connect to your database.

```toml
[databases.local]
engine = "postgres-psql"
spawn_database = "myapp"
spawn_schema = "_spawn"
environment = "dev"
command = { kind = "direct", direct = ["psql", "-U", "postgres", "myapp"] }
```

#### Docker Example

For databases running in Docker:

```toml
[databases.docker_local]
engine = "postgres-psql"
spawn_database = "myapp"
spawn_schema = "_spawn"
environment = "dev"
command = {
    kind = "direct",
    direct = [
        "docker", "exec", "-i", "myapp-db",
        "psql", "-U", "postgres", "myapp"
    ]
}
```

### 2. Provider Commands (Dynamic)

Provider commands are useful when the connection details need to be resolved dynamically, such as with cloud providers where connection setup is slow but the underlying connection is fast.

```toml
[databases.staging]
engine = "postgres-psql"
spawn_database = "myapp"
spawn_schema = "_spawn"
environment = "staging"
command = {
    kind = "provider",
    provider = [
        "gcloud", "compute", "ssh",
        "--zone", "us-central1-a",
        "my-sql-proxy-vm",
        "--project", "my-project",
        "--dry-run"
    ],
    append = ["-T", "sudo", "-u", "postgres", "psql", "myapp"]
}
```

The `provider` array specifies a command that outputs a shell command string to run. The `append` array contains additional arguments to append to the resolved command.

## Google Cloud SQL via SSH

### The Problem with Direct gcloud SSH

When connecting to Google Cloud SQL instances via SSH, using `gcloud compute ssh` directly works but is **significantly slower** because gcloud must.

This overhead happens **every time** spawn executes SQL, making migrations and tests much slower.

### Solution: Using the Provider Pattern

The provider pattern resolves the SSH command once using `gcloud compute ssh --dry-run`, then reuses the underlying SSH command directly for all subsequent operations.

#### Method 1: Test the Direct Approach (Slow)

You can use the gcloud command directly, but this will be called multiple times during a single `spawn migration apply` command.

```toml
[databases.staging_slow]
engine = "postgres-psql"
spawn_database = "myapp"
spawn_schema = "_spawn"
environment = "staging"
command = {
    kind = "direct",
    direct = [
        "gcloud", "compute", "ssh",
        "--zone", "us-central1-a",
        "my-sql-proxy-vm",
        "--project", "my-project",
        "--",
        "-T",
        "sudo", "-u", "postgres",
        "psql", "myapp"
    ]
}
```

#### Method 2: Use the Provider Pattern (Fast)

You can also use gcloud to provide the underlying ssh command needed to connect, which will be resolved just once, and then every connection to the database will use the provided ssh command directly.

```toml
[databases.staging]
engine = "postgres-psql"
spawn_database = "myapp"
spawn_schema = "_spawn"
environment = "staging"
command = {
    kind = "provider",
    provider = [
        "gcloud", "compute", "ssh",
        "--zone", "us-central1-a",
        "my-sql-proxy-vm",
        "--project", "my-project",
        "--dry-run"
    ],
    append = ["-T", "sudo", "-u", "postgres", "psql", "myapp"]
}
```

#### How It Works

1. **Provider Resolution**: When spawn initializes, it runs the `provider` command:

   ```bash
   gcloud compute ssh --zone us-central1-a my-sql-proxy-vm --project my-project --dry-run
   ```

2. **Parse Output**: Spawn parses the command output (gcloud outputs the underlying SSH command):

   ```
   ssh -t -i /path/to/key -o StrictHostKeyChecking=no user@ip
   ```

3. **Append Extra Arguments**: Spawn combines the resolved SSH command with the `append` args:

   ```bash
   ssh -t -i /path/to/key -o StrictHostKeyChecking=no user@ip -T sudo -u postgres psql myapp
   ```

4. **Reuse**: This final command is cached and reused for all SQL operations in that spawn session.

### Complete Example: Multiple Environments

```toml
# spawn.toml
spawn_folder = "spawn"
database = "local"  # Default database

# Local development (Docker)
[databases.local]
engine = "postgres-psql"
spawn_database = "myapp"
spawn_schema = "_spawn"
environment = "dev"
command = {
    kind = "direct",
    direct = ["docker", "exec", "-i", "myapp-db", "psql", "-U", "postgres", "myapp"]
}

# Staging (Google Cloud via provider - fast!)
[databases.staging]
engine = "postgres-psql"
spawn_database = "myapp"
spawn_schema = "_spawn"
environment = "staging"
command = {
    kind = "provider",
    provider = [
        "gcloud", "compute", "ssh",
        "--zone", "us-central1-a",
        "myapp-staging-proxy",
        "--project", "myapp-staging",
        "--dry-run"
    ],
    append = ["-T", "sudo", "-u", "postgres", "psql", "myapp"]
}

# Production (Google Cloud via provider - fast!)
[databases.production]
engine = "postgres-psql"
spawn_database = "myapp"
spawn_schema = "_spawn"
environment = "prod"
command = {
    kind = "provider",
    provider = [
        "gcloud", "compute", "ssh",
        "--zone", "us-east1-b",
        "myapp-prod-proxy",
        "--project", "myapp-production",
        "--dry-run"
    ],
    append = ["-T", "sudo", "-u", "postgres", "psql", "myapp"]
}
```

Usage:

```bash
# Use local database (default)
spawn migration apply <migration name>

# Use staging database
spawn --database staging migration apply <migration name>

# Use production database
spawn --database production migration apply <migration name>
```

## Advanced Configuration

### Multiple Databases

You can configure multiple databases and switch between them:

```bash
# Use specific database
spawn --database staging migration build my-migration

# Override in environment variable
export SPAWN_DATABASE=production
spawn migration apply
```

### Environment-Specific Templating

The `environment` field is available in your migration templates:

```sql
BEGIN;

{% if env == "dev" %}
-- Insert test data only in dev
INSERT INTO users (email) VALUES ('test@example.com');
{% endif %}

COMMIT;
```

## Security Considerations

- **Never commit sensitive credentials** to your `spawn.toml`
- Use environment variables or cloud authentication (like gcloud) instead of passwords in connection strings
- For production databases, consider using read-only connections for migration builds/tests
- The `spawn_schema` should have appropriate permissions for the user executing migrations
