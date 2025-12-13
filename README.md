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

# Features and plans

Here are some of my design goals with spawn:

### Core Migration Workflow

- [x] Handle history of functions/stored procs, so we can see proper history.
- [x] Ability to write custom hand-crafted migrations.
- [x] Plain SQL mostly, or rather generates plain SQL that can be modified.
- [x] Create empty migrations.
- [x] Variables supported, for substitution (`spawn run migration build 20240907212659-initial testvars.[json|toml|yamll]`, available under `variables` in templates).
- [ ] Idempotently apply migrations to database.
- [ ] Support for rollback scripts as an optional part of migrations.
- [ ] Repeatable migrations, including hashing the output (with variables perhaps) to check if it's been applied yet, and apply it if not.
- [ ] Migration dependencies, so that we can allow applying migrations out of order, but only if their dependencies have been applied.
- [ ] Mark a migration as draft, so it does not yet get applied to database.

### Database Interaction & Safety

- [ ] Migration status checking, to see what's been applied to a database.
- [ ] List migrations in database.
- [ ] Ability to apply specific migration or all.
  - [ ] Ensure database lock when doing so, where possible.
    - [ ] Advisory lock like sqitch has, to avoid multiple deployments all trying to apply the same migration at the same time: `pg_advisory_lock` etc.
- [ ] Allow for 'adopting' a migration, where you record in the database that it's been applied, without doing anything. Useful for if you're bringing in existing migrations from another system that have already been applied to the database.
- [ ] Store full schema changes applied in a migration table in database, so we have a record of what was done.
- [ ] Store variables used for applying a migration within the database migration table.
  - [ ] Allow encryption of variables in case they contain sensitive data.
- [ ] Store environment in Spawn database table in the target, so that you can't accidentally run a script with env set to `dev` and target `prod` with it. Spawn should check the target db to ensure it self reports as that env, and use that.
- [ ] Report on schema drift, comparing migrations vs some real database as it should be at given which migrations have been applied.

### Pinning and Component Management

- [ ] `spawn pin checkout <pin_hash>`: Restore just the `spawn` component files to the state they were in for a specific pin. This provides a focused way to inspect, debug, or modify historical migrations without affecting the entire project state like `git checkout` would.
- [ ] `spawn pin diff <migration1> <migration2>`: Show the exact changes to all shared components between two different migrations, providing a surgical diff for auditing and debugging.
- [ ] `spawn pin report --unused`: Scan the `/pinned` folder to find component objects no longer referenced by any migration, allowing for safe cleanup.
- [ ] `spawn pin validate`: Verify the integrity of all pinned objects, ensuring every object referenced in a migration's `lock.toml` exists and has the correct hash.
- [ ] Report on which components have changes that have never been included in a migration. Basically, check for the hash of that component and see if it's in any lock files, and if the migration includes that file in its SQL.

### Testing

- [x] `spawn test run <test>`: Allow creating a test script that you run against a database, which compares the output to expected output, and returns diff and exit status 1 if there's a difference.
- [x] `spawn test expect <test>` to generate the output expectations.
- [x] Option to run all tests (`spawn test compare`).
- [ ] Migration specific tests that run when migration is applied (similar to Sqitch).
- [ ] If useful, create helper functions like pgtap has, and optionally apply them to the database at test time, or to the copy that is used for tests.
- [ ] Handle deterministic migrations somehow with non-deterministic variables, particularly for tests where the same output is expected. E.g., `gen_uuid_v4` would return a new value on each invocation.
  - [ ] Interim option is to track when an undeterministic function is called, and optionally report on that when it's used as part of a test.
- [ ] Option to have spawn itself create the copy of the database with template, and exit before running psql commands if that fails.

### Multi-Tenancy and Advanced Architectures

- [ ] Easy to spin up new tenant schema.
- [ ] Easy to migrate each tenant schema.
- [ ] Allow a migration to have some parts that apply to shared schema, and some that apply to tenant schemas (e.g., via matrix). But even more complicated, allow us to reapply that change again, with different tenants, and it will only apply the tenant related changes to the new tenants, and not the shared schema changes.
- [ ] Allow migrations bundled in another package, like a framework. See [Multiple package migrations design](#multiple-package-migrations-design).
- [ ] Supporting migrations from multiple folders. E.g., if a separate project provided some of your migrations, then you can apply migrations from both folders.
- [ ] Flatten schema. E.g., deploy to local db with unique random values for variables (e.g., schema and user names), export again, and replace all references to the unique schema name with template variables again.
  - [ ] Optionally export the schema into a structured hierarchy of folders and files so that you can browse it easily on filesystem?

### Developer Experience & Tooling

- [ ] Watch a particular function or view, and re-apply automatically upon file change, to help with local testing.
  - [ ] Support a jinja template watch for local dev against local database, where if the rendered jinja template changes it gets re-applied. Useful in cases where we're updating views that depend on each other, and want to automatically recreate all those views as we edit files.
- [ ] Ability to preview in neovim and/or vscode the outputted sql, as you make changes to the migration template.
- [ ] If you have a view or function that depends on components that have changed, it would be nice to have a way to alert that the view or function should be recreated because it will now be different. Maybe via `pg_depend`.
- [ ] Enable writing scripts. We have migrations, and tests, but what if we want to run actions against a database that aren't part of a migration? E.g., to update, insert, or delete data for some test we are doing locally.
- [ ] SQL validation, perhaps similar to sqlx in Rust.
- [ ] Github action to call this easily in Github's CI/CD.

### Data & I/O

- [ ] Allow reading data from file types like csv's and use in templates, so you can loop over csv data to create insert(s), updates, whatever.
- [ ] Provide a way to import data from other sources? E.g., from a URL or script. Need to consider security implications.
- [ ] Handle secrets.
- [ ] Custom plugins or extensions.
