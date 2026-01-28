PostgreSQL migration management tool.

I like to lean heavily on the database, and therefore don't like to use tools that get in the way of that. There are plenty of other migration solutions out there, but this one solves problems that I've faced. A tool like sqitch was close to my needs, but didn't handle testing and reusing of components in the way I would ultimately like. Also, because of its dependence on PostgreSQL's variables, it prevented me from using variables in the way that I have been exploring for multi-tenant databases.

The main goal of Spawn is to make it easy to have reusable components, and keep track of changes as you modify your components over time, while sacrificing as little raw power of the database as possible. I'm not interested in abstractions, but rather tools that make it easier to work with the full breadth of features offered by modern databases.

Design goals:

- When updating functions or (some) views, you edit the file in the components folder for it in place, and reference that component in your new migration script. This ensures that git diffs will show what's changed in the function, rather than being a fresh copy.
- A record of the components used in a migration script are kept as they were at that time. This helps in case there is a need to return to an earlier migration script and update it for whatever reason. The version of the component at that time can be used, instead of the current version.
- Support for variables, so that things such as schema names can be configurable (e.g., generating migrations for multiple tenancies in a schema-per-tenant setup).
- Support plain PostgreSQL SQL, so that we aren't locked out of any database features.
- Track migrations that have been applied to database in a table.

For now, the focus is on PostgreSQL.

# Design

In your root project you will need a `spawn.toml` configuration file. It will point to a `spawn_folder` which is where your migrations, components, and tests go. In that folder we have:

1. `/components`. This contains standalone SQL snippets that can be modified and reused in migrations and tests. These are minijinja templates, which could contain just pure SQL. The goal is to have proper change tracking for these, so that we can look at the history in git for that file and see how it has changed over time.
1. `/migrations`. This folder contains your migrations, one folder for each. E.g., `20240802030220-support-roles/up.sql`. These are minijinja templates, designed to produce plain SQL migration scripts. In these templates, you can import components.
1. `/tests`. This contains all your tests, and works in a similar way to migrations, but with some helpful commands to work with them.
1. `/pinned`. This folder contains a copy of files as they were at a particular time that the migration was made stored by hash. It works in a similar way to git, and it is intended that you commit this folder to your repo. This allows migrations to be rerun/recreated as they were at that time, even if a referenced component has changed. Check [Pinning](#pinning) below for more details.

# Commands

Some commands available:

- `spawn init`
- `spawn migration new <name>` creates a new migration with a timestamp and the name.
- `spawn migration pin <migration>` pins the migration with the current components, creating objects in the `/pinned` folder and a `lock.toml` in the migration folder to point to the correct pinned files.
- `spawn migration build <migration> --pinned` builds the migration into the needed SQL. `--pinned` is optional, and is included when you want to use the pinned files for a reproducible migration.
- `spawn migration build <migration> --pinned` builds the migration into the needed SQL. `--pinned` is optional, and is included when you want to use the pinned files for a reproducible migration.
- `spawn test run <test>` will use psql (as configured in `spawn.toml` `psql_command` value) to pipe the generated test to your database. See [Testing](#testing) below for more details.

## Flags

- `--database` allows you to specify which specific database from the config to use. Defaults to `default_database` in your config.

# Examples

Printing out SQL:

```bash
spawn migration build 20240907212659-initial static/example.json
```

# Multiple package migrations design

Provide a standardised way for a package to expose its db migrations. Then, when another package with a binary imports it, it ensures there is a way to run the binary so that it outputs the migrations in the standardised way.

The spawn tool can then be configured to call that binary too, and operate in all the usual ways for things that make sense (e.g., can't pin?). So you can see which migrations are unapplied, and which have been applied, etc.

1. Framework has public function that returns embedded migrations
2. Project that imports the framework has a subcommand that outputs the migrations when invoked in an expected format, to the terminal.
3. Migrator can be told this command, and will allow you to run normal migration commands except treating the output from this binary as a pseudo file system rather than actual folder.

**Important**: Make sure this supports variables, and custom passing in of schema, etc.

TODO: think about how to handle ordering. Scenario: someone uses another package with its own migrations, version 2.3.0. It then does its own migrations for a while on its own schema, some of which interact with tables from the other package. Later, they update the upstream package from 2.3.0 to 3.1.4, where a number of migrations need to be applied. Those migrations should be considered to take place after any local migrations that have been done to date. How do we handle this? E.g., do we have a local file in repo that specifies what order to apply upstream package migrations? Maybe when you run it, it can check existing migrations, and specify new ones to be done at that point.

# Pinning

In order to ensure that earlier migrations can be run with the same source components, we support pinning. What this does is take a snapshot of the components folder as it is at that moment, and stores a reference to that snapshot in the migration folder. From then on, the migration can be run using the pinned version.

Under the hood, this works in a similar way to git, storing copies of files in the `/pinned` subfolder, with filenames matching their hash. The list of files and their hashes are stored as tree objects in the same `/pinned` folder, and the migration's config points to the root tree for its snapshot.

It is intended that you commit the `/pinned` folder to your repository.

To pin:

```bash
spawn migration pin <migration name>
```

# Testing

For now, we will support only postgres for testing. Testing will require psql to be available.

- Allow configuration of how to invoke psql.
- Provide scaffolding for automatic use of create database <x> with template <y>.
  - Configurable whether failed or successful tests tear down the test database or not.
- Follow the postgres testing style of producing an expected output, and then comparing future runs to that expected output with a diff.

# Stores

There are two different goals we want to achieve:

- Allow files to come from a variety of sources (filesystem, embedded in binary, remote server).
- Allow us to use either pinned components or current components.

Ideally then we can choose any combination of these two things. Our pin choice (none, spawn, git) should be a separate choice from our files/directories fetching method (local, embedded, remote).

# Features

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

### Configuration & Structure

- ✅ **TOML configuration** - `spawn.toml` for project settings
- ✅ **Organized folder structure** - `/components`, `/migrations`, `/tests`, `/pinned`
- ✅ **Database targeting** - `--database` flag for multiple database configurations
- ✅ **PostgreSQL focus** - Optimized for PostgreSQL features and workflows

# Roadmap

## Phase 1: Database Integration & Safety (Next Priority)

- ✅ **Migration application** - Idempotently apply migrations to database
- ✅ **Migration tracking** - Track applied migrations in database table
- ✅ **Migration status** - Check what migrations have been applied
- ✅ **Database locking** - Advisory locks to prevent concurrent migrations
- ✅ **Migration adoption** - Mark existing migrations as applied without running

## Phase 2: Enhanced Migration Features

- 🔄 **Rollback support** - Optional down scripts for migrations
- 🔄 **Repeatable migrations** - Hash-based detection for re-runnable migrations
- 🔄 **Migration dependencies** - Apply migrations out of order based on dependencies
- 🔄 **Draft migrations** - Mark migrations to exclude from database application
- 🔄 **Advanced scripting** - Run arbitrary commands during migration execution

## Phase 3: Enhanced Pinning & Component Management

- 🔄 **Pin checkout** - `spawn pin checkout <pin_hash>` to restore component states
- 🔄 **Pin diffing** - `spawn pin diff <migration1> <migration2>` between migrations
- 🔄 **Pin cleanup** - `spawn pin report --unused` to find orphaned objects
- 🔄 **Pin validation** - Verify integrity of all pinned objects
- 🔄 **Component change tracking** - Report components with unapplied changes
- 🔄 **Environment-specific pinning** - Per-environment pin requirements

## Phase 4: Multi-Tenancy & Advanced Architectures

- 🔄 **Tenant schema management** - Easy tenant schema creation and migration
- 🔄 **Mixed schema migrations** - Apply parts to shared vs tenant schemas
- 🔄 **Package migration support** - Import migrations from external packages
- 🔄 **Multi-folder migrations** - Support migrations from multiple sources
- 🔄 **Schema flattening** - Export/import with variable substitution

## Phase 5: Developer Experience & Tooling

- 🔄 **File watching** - Auto-apply changes for local development
- 🔄 **Live preview** - Real-time SQL preview in editors (Neovim/VSCode)
- 🔄 **Dependency tracking** - Alert when components need recreation
- 🔄 **Script execution** - Run ad-hoc database scripts outside migrations
- 🔄 **SQL validation** - Static analysis similar to sqlx
- 🔄 **GitHub Actions** - Official CI/CD integration

## Phase 6: Enhanced Testing & Safety

- 🔄 **Migration-specific tests** - Tests that run when migrations are applied
- 🔄 **Helper functions** - Optional pgTAP-style testing utilities
- 🔄 **Deterministic handling** - Manage non-deterministic functions in tests
- 🔄 **Automatic test databases** - Spawn-managed test database creation
- 🔄 **Schema drift detection** - Compare expected vs actual database state
- 🔄 **Variable encryption** - Secure storage of sensitive migration variables

## Phase 7: Data & I/O Integration

- 🔄 **CSV/data file support** - Import and loop over data files in templates
- 🔄 **External data sources** - Import from URLs and external scripts
- 🔄 **Secret management** - Secure handling of sensitive data
- 🔄 **Plugin system** - Custom extensions and plugins

## Legend

- ✅ **Complete** - Feature is implemented and available
- 🚧 **In Progress** - Currently being developed
- 🔄 **Planned** - Scheduled for future development

# Testing

As we are using opendal for the filesystem, we can take advantage of its memory storage to run our tests. Therefore, a lot of tests will involve creating files within the memory storage, and inspecting it there, and also have automatic cleanup at the end.

## Running Tests

### Unit Tests

Unit tests use in-memory storage and don't require any external dependencies:

```bash
cargo test --lib --bins
cargo test --test migration_build
```

### Integration Tests (PostgreSQL)

Integration tests require a running PostgreSQL instance. They are marked with `#[ignore]` so they don't run during normal `cargo test`.

**Local development (with Docker):**

1. Start the PostgreSQL container:

   ```bash
   docker compose up -d
   ```

2. Run the integration tests:
   ```bash
   cargo test --test integration_postgres -- --ignored
   ```

**CI mode (direct PostgreSQL connection):**

Set environment variables for direct psql connection:

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

**Keeping test databases for inspection:**

By default, test databases are dropped after each test. Set `SPAWN_TEST_KEEP_DB` to preserve them:

```bash
SPAWN_TEST_KEEP_DB=1 cargo test --test integration_postgres test_migration_creates_table -- --ignored --nocapture
```

This will print the database name and connection instructions so you can inspect the results.

### Test Isolation

Each integration test creates its own unique database, allowing tests to run in parallel without interference. The test databases are automatically cleaned up after each test completes.

### Running All Tests

```bash
# Run unit tests only (fast, no dependencies)
cargo test

# Run everything including integration tests
docker compose up -d
cargo test -- --ignored
```
