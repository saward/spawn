## Core Migration Workflow

- [x] Handle history of functions/stored procs, so we can see proper history.
- [x] Ability to write custom hand-crafted migrations.
- [x] Plain SQL mostly, or rather generates plain SQL that can be modified.
- [x] Create empty migrations.
- [x] Variables supported, for substitution (`spawn run migration build 20240907212659-initial testvars.[json|toml|yamll]`, available under `variables` in templates).
- [x] Escape variables by default so that injection attacks are harder.
- [ ] Idempotently apply migrations to database.
- [ ] Support for rollback scripts as an optional part of migrations.
- [ ] Repeatable migrations, including hashing the output (with variables perhaps) to check if it's been applied yet, and apply it if not.
- [ ] Migration dependencies, so that we can allow applying migrations out of order, but only if their dependencies have been applied.
- [ ] Mark a migration as draft, so it does not yet get applied to database.
- [ ] Allow advanced scripts that can run arbitrary commands at points within the migration to, e.g., update or pull from an external data source before proceeding further?

## Database Interaction & Safety

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

## Pinning and Component Management

- [ ] `spawn pin checkout <pin_hash>`: Restore just the `spawn` component files to the state they were in for a specific pin. This provides a focused way to inspect, debug, or modify historical migrations without affecting the entire project state like `git checkout` would.
- [ ] `spawn pin diff <migration1> <migration2>`: Show the exact changes to all shared components between two different migrations, providing a surgical diff for auditing and debugging.
- [ ] `spawn pin report --unused`: Scan the `/pinned` folder to find component objects no longer referenced by any migration, allowing for safe cleanup.
- [ ] `spawn pin validate`: Verify the integrity of all pinned objects, ensuring every object referenced in a migration's `lock.toml` exists and has the correct hash.
- [ ] Report on which components have changes that have never been included in a migration. Basically, check for the hash of that component and see if it's in any lock files, and if the migration includes that file in its SQL.
- [ ] Allow specifying per-env pin requirements. Not required, prompt, and required.

## Testing

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

## Developer Experience & Tooling

- [ ] Watch a particular function or view, and re-apply automatically upon file change, to help with local testing.
  - [ ] Support a jinja template watch for local dev against local database, where if the rendered jinja template changes it gets re-applied. Useful in cases where we're updating views that depend on each other, and want to automatically recreate all those views as we edit files.
- [ ] Ability to preview in neovim and/or vscode the outputted sql, as you make changes to the migration template.
- [ ] If you have a view or function that depends on components that have changed, it would be nice to have a way to alert that the view or function should be recreated because it will now be different. Maybe via `pg_depend`.
- [ ] Enable writing scripts. We have migrations, and tests, but what if we want to run actions against a database that aren't part of a migration? E.g., to update, insert, or delete data for some test we are doing locally.
- [ ] SQL validation, perhaps similar to sqlx in Rust.
- [ ] Github action to call this easily in Github's CI/CD.

## Data & I/O

- [ ] Refactor the migration execution to use tokio::task::spawn_blocking with the synchronous postgres crate, using SyncIoBridge to convert the async OpenDAL input into a synchronous stream. This restores deterministic RAII transaction rollback (Drop safety) while maintaining low-memory streaming from S3 through Minijinja. No need to store entire input or output in memory at one time.
- [ ] Allow reading data from file types like csv's and use in templates, so you can loop over csv data to create insert(s), updates, whatever.
- [ ] Provide a way to import data from other sources? E.g., from a URL or script. Need to consider security implications.
- [ ] Handle secrets.
- [ ] Custom plugins or extensions.
