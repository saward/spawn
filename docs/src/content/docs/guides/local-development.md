---
title: Local Development
description: How to set up and run Spawn locally for development and testing.
---

This guide covers setting up a local development environment for working with Spawn, including running PostgreSQL via Docker, executing tests, and inspecting test databases.

## Prerequisites

- [Spawn installed](/getting-started/install/)
- Docker (for running PostgreSQL locally)

## Starting PostgreSQL

This project has a docker-compose file that you can use for integration testing:

```bash
docker compose up -d
```

## Running Tests

Spawn has two categories of tests: unit tests, some of which use in-memory storage and require no external dependencies, and integration tests that require a running PostgreSQL instance.

### Unit Tests

Unit tests use in-memory storage via opendal and don't need a database:

```bash
cargo test
```

Or to run all integration tests, which require the local database running

```bash
cargo test -- --ignored
```

### Integration Tests

Integration tests are marked with `#[ignore]` so they don't run during a normal `cargo test`. They require a running PostgreSQL instance.

**With Docker (recommended for local development):**

```bash
docker compose up -d
cargo test --test integration_postgres -- --ignored
```

**With a direct PostgreSQL configuration:**

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

## Test Isolation

Each integration test creates its own unique database, so tests can run without interference. Databases are automatically dropped when each test completes.

## Keeping Test Databases for Inspection

By default, test databases are cleaned up after each test. To preserve them for manual inspection, set `SPAWN_TEST_KEEP_DB`:

```bash
SPAWN_TEST_KEEP_DB=1 cargo test --test integration_postgres test_migration_creates_table -- --ignored --nocapture
```

This prints the database name and connection instructions so you can connect and inspect the results with `psql`.
