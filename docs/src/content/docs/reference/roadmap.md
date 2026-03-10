---
title: Roadmap
description: Planned features and future direction for Spawn.
---

This is a high-level overview of where Spawn is headed. Items here are subject to change.

## Currently Available

### Core Migration Management

- ✅ **Project initialization** - `spawn init` to set up new projects
- ✅ **Migration creation** - `spawn migration new <name>` creates timestamped migrations
- ✅ **Migration building** - `spawn migration build` generates SQL from templates
- ✅ **Component system** - Reusable SQL snippets with proper git history tracking
- ✅ **Variable substitution** - Support for JSON/TOML/YAML variable files in templates
- ✅ **Minijinja templating** - Full templating support for migrations and components

### Pinning System

- ✅ **Migration pinning** - `spawn migration pin <migration>` creates reproducible snapshots
- ✅ **Git-like object storage** - Hash-based storage of pinned components in `/pinned` folder
- ✅ **Reproducible builds** - `--pinned` flag uses locked component versions

### Testing Framework

- ✅ **Test execution** - `spawn test run <test>` runs tests against database
- ✅ **Expected output comparison** - PostgreSQL-style diff-based testing
- ✅ **Test expectation generation** - `spawn test expect <test>` creates baseline outputs
- ✅ **Batch test running** - `spawn test compare` runs all tests
- ✅ **GitHub Actions** - Official CI/CD integration

### Configuration & Structure

- ✅ **TOML configuration** - `spawn.toml` for project settings
- ✅ **Organized folder structure** - `/components`, `/migrations`, `/tests`, `/pinned`
- ✅ **Target selection** - `--target` flag for multiple target configurations
- ✅ **PostgreSQL focus** - Optimized for PostgreSQL features and workflows

## Roadmap

### Database Integration & Safety (Next Priority)

- ✅ **Migration application** - Idempotently apply migrations to database
- ✅ **Migration tracking** - Track applied migrations in database table
- ✅ **Migration status** - Check what migrations have been applied
- ✅ **Database locking** - Advisory locks to prevent concurrent migrations
- ✅ **Migration adoption** - Mark existing migrations as applied without running

### Enhanced Migration Features

- 🔄 **Rollback support** - Optional down scripts for migrations
- 🔄 **Repeatable migrations** - Hash-based detection for re-runnable migrations
- 🔄 **Migration dependencies** - Apply migrations out of order based on dependencies
- 🔄 **Draft migrations** - Mark migrations to exclude from database application
- 🔄 **Advanced scripting** - Run arbitrary commands during migration execution
- 🔄 **Custom commands/callbacks** - Run arbitrary commands before or after migrations or tests

### Enhanced Pinning & Component Management

- 🔄 **Pin checkout** - `spawn pin checkout <pin_hash>` to restore component states
- 🔄 **Pin diffing** - `spawn pin diff <migration1> <migration2>` between migrations
- 🔄 **Pin cleanup** - `spawn pin report --unused` to find orphaned objects
- 🔄 **Pin validation** - Verify integrity of all pinned objects
- 🔄 **Component change tracking** - Report components with unapplied changes
- 🔄 **Environment-specific pinning** - Per-environment pin requirements

### Multi-Tenancy & Advanced Architectures

- 🔄 **Tenant schema management** - Easy tenant schema creation and migration
- 🔄 **Mixed schema migrations** - Apply parts to shared vs tenant schemas
- 🔄 **Package migration support** - Import migrations from external packages
- 🔄 **Multi-folder migrations** - Support migrations from multiple sources
- 🔄 **Schema flattening** - Export/import with variable substitution

### Developer Experience & Tooling

- 🔄 **File watching** - Auto-apply changes for local development
- 🔄 **Live preview** - Real-time SQL preview in editors (Neovim/VSCode)
- 🔄 **Dependency tracking** - Alert when components need recreation
- 🔄 **Script execution** - Run ad-hoc database scripts outside migrations
- 🔄 **SQL validation** - Static analysis similar to sqlx

### Enhanced Testing & Safety

- 🔄 **Migration-specific tests** - Tests that run when migrations are applied
- 🔄 **Helper functions** - Optional pgTAP-style testing utilities
- 🔄 **Deterministic handling** - Manage non-deterministic functions in tests
- 🔄 **Automatic test databases** - Spawn-managed test database creation
- 🔄 **Schema drift detection** - Compare expected vs actual database state
- 🔄 **Variable encryption** - Secure storage of sensitive migration variables
- 🔄 **Separate simultaneous scripts** - Allow running SQL in a separate connection during test (see below)

### Data & I/O Integration

- 🔄 **Support remote storage** - Run migrations from another source (S3, etc)
- 🔄 **CSV/data file support** - Import and loop over data files in templates
- 🔄 **External data sources** - Import from URLs and external scripts
- 🔄 **Secret management** - Secure handling of sensitive data
- 🔄 **Plugin system** - Custom extensions and plugins

### Legend

- ✅ **Complete** - Feature is implemented and available
- 🚧 **In Progress** - Currently being developed
- 🔄 **Planned** - Scheduled for future development

## Details

### Separate simultaneous scripts

NOTE: The below may not work if we just look for something being pushed to writer. We need a way to ensure that postgres has actually processed the command, so maybe we can have a write to a table or setting, and the new thread monitors for that value being written before proceeding. Can we use LISTEN/NOTIFY?

minijinja appears to process templates in order. It would be nice to allow the running of separate scripts during a test, on a separate connection/session, in order to test interactions between sessions/connections. E.g., one claiming a lock while another tries to perform an action while the lock is active.

One possible way to do this is to have custom minijinja functions like `{% bg("script.sql") %}` that then waits until the current minijinja writes have finished, then it spawns a separate minijinja template and starts streaming it to a separate psql connection. Perhaps a `bg` method that just runs immediately in the background, while another `wait` one waits until it finishes before returning.
