# CLAUDE.md

## Project Overview

Spawn is a database migration tool written in Rust, focused on PostgreSQL initially, connecting via psql rather than direct protocol. It emphasises raw SQL power over ORM abstractions, with reusable components, reproducible builds via content-addressed pinning, and minijinja templating.

## Key Concepts

- **Migrations**: Timestamped SQL scripts in `migrations/YYYYMMDDHHMMSS-name/up.sql`. These are minijinja templates that can include components.
- **Components**: Reusable SQL snippets in `components/`. Included in migrations, tests and scripts via `{% include "component.sql" %}`. Never an entry point themselves.
- **Pinning**: Snapshots components into a content-addressed store (`pinned/`) and records a `pin` in the migration's `lock.toml` (or the script's `config.toml`). This locks a migration to the exact component versions used at pin time, so re-running it later produces identical SQL. Uses xxhash3_128 for content hashing. The `pin` key is optional in both files, and both are rewritten in place so declared variables survive pinning — so "pinned" means `pin` is set, never merely that the file exists. `has_lock_toml`/`has_config_toml` answer only whether the file is there.
- **Variables**: Template variables from JSON/TOML/YAML files, accessible as `{{ variables.key }}`. A migration may declare its own in `lock.toml`, and a script in `config.toml` (`source = "value"` or `source = "prompt"`, mirroring secrets); declarations are the lowest-priority source, losing to both `--variables` and `--var-json`. `migration build|apply` and `script run|build` also take repeatable `--var-json key=<json>` overrides and `--use-defaults`. `migration apply` prepares the whole batch before applying any of it: every pin is checked first, then every declaration resolved, so an unpinned migration refuses the batch before any value is prompted for and a prompt can never surface mid-batch. The value is parsed as JSON so it keeps its type, and is transcoded into the variables file's own model — a TOML file stays TOML and is never routed through JSON.
- **Scripts**: Standalone SQL in `scripts/<name>/script.sql`, run by `spawn script run <name>` — operational SQL rather than schema change. A sibling of `migrations/` and `tests/`, deliberately *not* under `components/`: a script inside the component tree would change the root hash every migration's pin records. Pinning is opt-in and automatic once taken: `spawn script pin` records a `pin` in `scripts/<name>/config.toml`, and from then on every run and build resolves against that tree without a flag (`--no-pin` opts back out, `--pinned` asserts a pin exists). That file may also exist *without* a pin, declaring `[variables]` only — so "pinned" means the `pin` key is set, never merely that `config.toml` exists. `spawn script pin` rewrites the file in place so declarations survive. A pin freezes the components a script includes but **not** `script.sql` itself, which is always read live. Each run appends a row to `_spawn.script_run`; nothing about a later run is gated on that history.

## Project Structure

```
src/
  main.rs              # Entry point & telemetry
  cli.rs               # Clap CLI definitions
  config.rs            # spawn.toml loading, path helpers
  migrator.rs          # Migration build orchestrator
  template.rs          # Minijinja setup, streaming generation
  script.rs            # Running a standalone script
  commands/
    migration/         # new, build, pin, apply, adopt, status
    test/              # new, build, run, compare, expect
    script/            # new, run, build, pin
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
- **Streaming**: Templates render directly to a writer (piped to psql stdin) without materialising the full SQL in memory. The `migration_history.checksum` audit-trail fingerprint is a hash of the migration's raw (unrendered) template source (`StreamingGeneration::raw_checksum`), computed separately from the streamed render — never the rendered/executed SQL, since that may contain resolved secret values.
- **Advisory locking**: Prevents concurrent migration application via `pg_try_advisory_lock`.
- **Command pattern**: Each CLI command is a struct implementing `Command` trait with `execute(&self, config) -> Result<Outcome>`.
- **Storage**: Uses `opendal::Operator` for filesystem abstraction. Tests use in-memory operators.
- **Undefined variables**: `template_env` sets minijinja to `UndefinedBehavior::SemiStrict`, so printing or iterating a variable that was never set is a render error naming the file and line, rather than an empty string. A variable set to an empty value is unaffected. Truthiness tests (`{% if variables.x %}`) and `| default(..)` still work, which is why SemiStrict rather than Strict.
- **SQL escaping**: The `sql_query!` macro and `EscapedLiteral`/`EscapedIdentifier` types prevent SQL injection. Use these instead of string formatting for any user-provided values.

## CLI Structure

```
spawn init
spawn migration new|pin|build|apply|adopt|status
spawn test new|build|run|compare|expect
spawn script new|run|build|pin
```

Key script run flags: `--pinned` (require a pin; error if unpinned), `--no-pin` (ignore an existing pin), `--var-json` (ad-hoc typed variables), `--use-defaults` (take declared defaults instead of prompting), `--yes` (skip confirmation; also suppresses variable prompts). `run` and `build` both pin automatically when the script has a `config.toml`, and report the tree they used on stderr.

Key apply flags: `--no-pin` (skip pin requirement), `--retry` (retry failed migration), `--yes` (skip confirmation; also suppresses variable prompts), `--use-defaults` (take declared defaults instead of prompting), `--reuse-connection` (reuse database connection across migrations).

## Documentation

User-facing documentation lives in `docs/src/content/docs/`. When adding or changing CLI commands or flags:

1. Update the relevant `.mdx` file in `docs/src/content/docs/cli/` (e.g., `migration-apply.mdx`).
2. Add new flags to the `options` array in the `<CLICommand>` component.
3. Document behavior in a dedicated section if the flag has non-trivial semantics.
4. Add an example in the "Examples" section showing typical usage.

When updating CLI tool or other docs, ensure that you also update the sidebar in `docs/astro.config.mjs`.

## Conventions

- Migration apply defaults to `pinned: true`. Unpinned migrations must use `--no-pin`; a `lock.toml` that declares variables but records no `pin` counts as unpinned.
- Integration tests are `#[ignore]` and run separately with `-- --ignored`.
- Test helpers: `MigrationTestHelper` (in-memory builds), `IntegrationTestHelper` (full DB lifecycle).
- Error types for migration operations are in `MigrationError` enum (`src/engine/mod.rs`).
- The `_spawn` schema is used by default for all internal tracking tables.
- Configuration of a local setup is via a `spawn.toml` file for each project.
