---
title: Database Connections
description: How to configure database connections in spawn.toml
---

## Overview

Spawn requires a database connection to apply migrations and run tests. Database connections are configured in your `spawn.toml` file under the `[databases]` section.

## Basic Configuration

Each database configuration requires:

- `engine`: The database engine type (currently only `"postgres-psql"`)
- `spawn_database`: The database name to connect to
- `spawn_schema`: The schema where spawn stores migration tracking (default: `"_spawn"`)
- `environment`: Environment name (e.g., `"dev"`, `"staging"`, `"prod"`)
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

When connecting to Google Cloud SQL instances via SSH, using `gcloud compute ssh` directly works but is **significantly slower** because gcloud must:

1. Look up the instance details
2. Retrieve SSH keys
3. Configure SSH options
4. Establish the connection

This overhead happens **every time** spawn executes SQL, making migrations and tests painfully slow.

### Solution: Using the Provider Pattern

The provider pattern resolves the SSH command once using `gcloud compute ssh --dry-run`, then reuses the underlying SSH command directly for all subsequent operations.

#### Step 1: Test the Direct Approach (Slow)

First, let's see the slow approach for comparison:

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

**Performance**: ~2-4 seconds per SQL execution (slow!)

#### Step 2: Use the Provider Pattern (Fast)

Now configure spawn to use the provider pattern:

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

**Performance**: ~0.1-0.3 seconds per SQL execution (fast!)

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

### Testing Your Provider Setup

You can test the provider command directly to see what SSH command it generates:

```bash
gcloud compute ssh --zone us-central1-a my-sql-proxy-vm --project my-project --dry-run
```

Expected output (the underlying SSH command):

```
ssh -t -i /home/user/.ssh/google_compute_engine -o UserKnownHostsFile=/home/user/.ssh/google_compute_known_hosts user@35.123.45.67
```

Spawn will parse this output and append your `append` arguments to create the final command.

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
spawn migration apply

# Use staging database
spawn --database staging migration apply

# Use production database
spawn --database production migration apply
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

## Troubleshooting

### Testing Connection

Test your database connection:

```bash
spawn --database mydb migration build some-migration
```

If this succeeds, your connection is configured correctly.

### Google Cloud SSH Issues

If the provider isn't working:

1. Test the provider command directly to ensure it outputs a valid SSH command:

   ```bash
   gcloud compute ssh --zone ZONE INSTANCE --project PROJECT --dry-run
   ```

   This should output an SSH command like:

   ```
   ssh -t -i /path/to/key -o ... user@ip
   ```

2. Ensure you're authenticated:

   ```bash
   gcloud auth login
   ```

### Docker Connection Issues

If Docker connections fail:

1. Verify container is running:

   ```bash
   docker ps | grep your-container
   ```

2. Test psql directly:
   ```bash
   docker exec -i your-container psql -U postgres dbname -c "SELECT 1;"
   ```

## Security Considerations

- **Never commit sensitive credentials** to your `spawn.toml`
- Use environment variables or cloud authentication (like gcloud) instead of passwords in connection strings
- For production databases, consider using read-only connections for migration builds/tests
- The `spawn_schema` should have appropriate permissions for the user executing migrations
