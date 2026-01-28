# CLAUDE.md

## Project Overview

Spawn is a database migration tool written in Rust, focused on PostgreSQL initially, connecting via psql rather than direct protocol. It emphasises raw SQL power over ORM abstractions, with reusable components, reproducible builds via content-addressed pinning, and minijinja templating.

## Key Concepts

- **Migrations**: Timestamped SQL scripts in `migrations/YYYYMMDDHHMMSS-name/up.sql`. These are minijinja templates that can include components.
- **Components**: Reusable SQL snippets in `components/`. Included in migrations via `{% include "component.sql" %}`.
- **Pinning**: Snapshots components into a content-addressed store (`pinned/`) and writes a `lock.toml` in the migration folder. This locks a migration to the exact component versions used at pin time, so re-running it later produces identical SQL. Uses xxhash3_128 for content hashing.
- **Variables**: Template variables from JSON/TOML/YAML files, accessible as `{{ variables.key }}`.

## Project Structure

```
src/
  main.rs              # Entry point & telemetry
  cli.rs               # Clap CLI definitions
  config.rs            # spawn.toml loading, path helpers
  migrator.rs          # Migration build orchestrator
  template.rs          # Minijinja setup, streaming generation
  commands/
    migration/         # new, build, pin, apply, adopt, status
    test/              # new, build, run, compare, expect
  engine/
    mod.rs             # Engine trait, error types, MigrationStatus
    postgres_psql.rs   # PostgreSQL driver via psql CLI
  store/
    pinner/            # CAS pinning system (Latest, Spawn pinners)
  escape.rs            # SQL escaping (EscapedLiteral, EscapedIdentifier, sql_query!)
  sql_formatter/       # Dialect-specific escaping
tests/
  migration_build.rs   # In-memory migration build tests
  integration_postgres.rs  # PostgreSQL integration tests (require DB)
static/
  example/             # Example project used by `spawn init`
  engine-migrations/   # Internal schema migrations for _spawn tables
```

## Build & Test Commands

```bash
cargo build                                          # Build
cargo test                                           # Unit tests + doc tests
cargo test -- --ignored                              # Integration tests (requires PostgreSQL running)
cargo test test_name -- --ignored --nocapture         # Single integration test with output
```

## Architecture Notes

- **Engine trait** (`src/engine/mod.rs`): Async trait for database operations. Currently only `PSQL` (postgres via psql CLI). Migration apply uses two separate psql sessions: one runs the migration, the second records the outcome (success or failure) to `_spawn.migration_history`.
- **Streaming**: Templates render directly to a writer (piped to psql stdin) without materialising the full SQL in memory. A `TeeWriter` computes checksums during streaming.
- **Advisory locking**: Prevents concurrent migration application via `pg_try_advisory_lock`.
- **Command pattern**: Each CLI command is a struct implementing `Command` trait with `execute(&self, config) -> Result<Outcome>`.
- **Storage**: Uses `opendal::Operator` for filesystem abstraction. Tests use in-memory operators.
- **SQL escaping**: The `sql_query!` macro and `EscapedLiteral`/`EscapedIdentifier` types prevent SQL injection. Use these instead of string formatting for any user-provided values.

## CLI Structure

```
spawn init
spawn migration new|pin|build|apply|adopt|status
spawn test new|build|run|compare|expect
```

Key apply flags: `--no-pin` (skip pin requirement), `--retry` (retry failed migration), `--yes` (skip confirmation).

## Conventions

- Migration apply defaults to `pinned: true`. Unpinned migrations must use `--no-pin`.
- Integration tests are `#[ignore]` and run separately with `-- --ignored`.
- Test helpers: `MigrationTestHelper` (in-memory builds), `IntegrationTestHelper` (full DB lifecycle).
- Error types for migration operations are in `MigrationError` enum (`src/engine/mod.rs`).
- The `_spawn` schema is used by default for all internal tracking tables.
- Configuration of a local setup is via a `spawn.toml` file for each project.
