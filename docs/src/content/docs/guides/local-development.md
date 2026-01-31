---
title: Local Development
description: How to set up and run Spawn locally for development and testing.
---

This guide covers setting up a local development environment for working with Spawn, including running PostgreSQL via Docker, executing tests, and inspecting test databases.

## Prerequisites

- [Spawn installed](/getting-started/install/)
- Docker (for running PostgreSQL locally)

## Starting PostgreSQL

If you initialized your project with `spawn init --docker`, you already have a `docker-compose.yaml`. Start the database:

```bash
docker compose up -d
```

This starts a PostgreSQL 17 instance accessible on the port configured in your compose file.

## Running Tests

Spawn has two categories of tests: unit tests that use in-memory storage and require no external dependencies, and integration tests that require a running PostgreSQL instance.

### Unit Tests

Unit tests use in-memory storage via opendal and don't need a database:

```bash
cargo test --lib --bins
cargo test --test migration_build
```

Or to run all non-integration tests:

```bash
cargo test
```

### Integration Tests

Integration tests are marked with `#[ignore]` so they don't run during a normal `cargo test`. They require a running PostgreSQL instance.

**With Docker (recommended for local development):**

```bash
docker compose up -d
cargo test --test integration_postgres -- --ignored
```

**With a direct PostgreSQL connection:**

Set environment variables to point at your PostgreSQL instance:

```bash
SPAWN_TEST_PSQL_HOST=localhost \
SPAWN_TEST_PSQL_PORT=5432 \
SPAWN_TEST_PSQL_USER=spawn \
PGPASSWORD=spawn \
cargo test --test integration_postgres -- --ignored
```

**Running a specific integration test:**

```bash
cargo test --test integration_postgres test_migration_is_idempotent -- --ignored --nocapture
```

### Running All Tests

```bash
docker compose up -d
cargo test -- --ignored
```

## Test Isolation

Each integration test creates its own unique database, so tests can run in parallel without interference. Databases are automatically dropped when each test completes.

## Keeping Test Databases for Inspection

By default, test databases are cleaned up after each test. To preserve them for manual inspection, set `SPAWN_TEST_KEEP_DB`:

```bash
SPAWN_TEST_KEEP_DB=1 cargo test --test integration_postgres test_migration_creates_table -- --ignored --nocapture
```

This prints the database name and connection instructions so you can connect and inspect the results with `psql`.

## CI Configuration

In CI environments where Docker may not be available, connect to PostgreSQL directly via environment variables:

```bash
SPAWN_TEST_PSQL_HOST=localhost \
SPAWN_TEST_PSQL_PORT=5432 \
SPAWN_TEST_PSQL_USER=spawn \
PGPASSWORD=spawn \
cargo test --test integration_postgres -- --ignored
```

Set these variables in your CI provider's secrets or environment configuration as appropriate.
