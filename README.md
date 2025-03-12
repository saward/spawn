PostgreSQL migration management tool.

I like to lean heavily on the database.  That means I don't like to use tools that get in the way of that, stopping me from doing what I want.  That being said, working with the database could benefit from better tooling.

There are plenty of other migration solutions out there, but this one's mine.  The main alternative I've considered is sqitch, and while it seems to work pretty well, I've recently been looking at how to handle multiple tenants in a PostgreSQL database with a schema-per-tenant setup.  That means that I want to run the migrations per schema.  Unfortunately, sometimes I find that I need to mention specific names of the schemas or users, and when using psql's variables to substitute those in with sqitch, I hit unexpected limitations.

Here are some of my design goals with migrator:

- Handle history of functions/stored procs, so we can see proper history.
- Ability to write custom hand-crafted migrations.
- Support for rollback scripts as an optional feature.
- Migration status checking, to see what's been applied to a database.
- Easy to spin up new tenant schema.
- Easy to migrate each tenant schema.
- Supporting migrations from multiple folders.  E.g., if a separate project provided some of your migrations, then you can apply migrations from both folders.
- Plain SQL mostly, or rather generates plain SQL that can be modified.
- Find a good way for testing SQL/unit testing.
- Store full schema changes applied in a migration table in database, so we have a record of what was done.
- Variables supported, for substitution, as well as matrices to generate migrations for a bunch of sites.  Or maybe we never generate and store the files, since there are many tenants, and instead run them against each schema somehow without generating stored/saved files for each schema.
- Allow a migration to have some parts that apply to shared schema, and some that apply to tenant schemas (e.g., via matrix).  But even more complicated, allow us to reapply that change again, with different tenants, and it will only apply the tenant related changes to the new tenants, and not the shared schema changes.
- Keep track of which migrations have been applied, so that when targeting a schema it will check which need to be applied and then apply all.
- Handle secrets
- Watch a particular function or view, and re-apply automatically upon file change, to help with local testing.
 - Support a jinja template watch for local dev against local database, where if the rendered jinja template changes it gets re-applied.  Useful in cases where we're updating views that depend on each other, and want to automatically recreate all those views as we edit files.
- Stretch goals:
 - Some clever way to watch changes in the view/function folder, and automatically update.  Functions are easier, but views will fail when columns change or they have dependencies.
  - I've tried having a schema dedicated to these things that are easy to throw away and rectrate, but the two problems are (a) it can get slow when there's more, making it hard to do in transactoin, and (b) I suspect we'll hit cases where can't be fully done inside transaction or rolled back.
 - Handle migration of views properly (e.g., when they depend on each other).
 - Reverting.  For now, likely this will assume you're performing DDL in a transaction in most cases.  Later, want to support something more official, particularly for cases where transactions are not possible or feasible.
 - Flatten schema.  E.g., deploy to local db with unique random values for variables (e.g., schema and user names), export again, and replace all references to the unique schema name with template variables again.
 - Examine view dependencies, so that when these are updated we can check if the child views need to be deleted and recreated.
 - SQL validation, perhaps similar to sqlx in Rust.
 - Custom plugins or extensions.
 - Inspect postgresql to learn dependencies of views, to make it easy to drop and recreate exactly the ones needed when creating a new migration.

# Design

We have three primary folders:

1. `<base>/migrator/components`.  This contains standalone SQL snippets that can be modified and reused.  These are minijinja templates, but they could be plain SQL.  The goal is to have proper change tracking for these, so that we can look at the history in git for that file and see how it has changed over time.
  - A subfolder may be `<base>/migrator/idempotent_schemas`, containing schemas that are safe to destroy and reapply.  They operate the same as components for now, but one day we may add special functionality around them.
2. `<base>/migrator/migrations`.  This folder contains subfolders, one for each migration, and those folders contain the migration script.  E.g., `20240802030220-support-roles/up.sql.jinja`.  These are minijinja templates, designed to produce plain SQL migration scripts.  In these templates, you can import components.
   - Containes a `pinned_files` file, which is a list of file names to their sha256 pinned file (see 3. below).
3. `<base>/migrator/migrations/pinned_components`.  This folder contains a copy of files as they were at a particular time that the migration was made.  This allows the migration to be rerun/recreated even if the referenced file has changed.  Each migration, when pinned, creates (if it does not exist) a file for each referenced component, whose name is its sha256 sum, and whose location is within a subfolder with a prefix of the first two character.  E.g., `c8/c8fa8f7395e8e0c5e6a457a7c6cd4a1adf87e09cbcc99aa683b0c2eea7368a89`.  The `pinned_files` for the migration may then include `c8fa8f7395e8e0c5e6a457a7c6cd4a1adf87e09cbcc99aa683b0c2eea7368a89 mycomponent.sql` as an entry.

Design goals:

- When updating functions or (some) views, you edit the file in the components folder for it in place, and reference that component in your new migration script.  This ensures that git diffs will show what's changed in the function, rather than being a fresh copy.
- A record of the components used in a migration script are kept as they were at that time.  This helps in case there is a need to return to an earlier migration script and update it for whatever reason.  The version of the component at that time can be used, instead of the current version.
- Support for variables, so that things such as schema names can be configurable (e.g., generating migrations for multiple tenancies in a schema-per-tenant setup).
- Support plain PostgreSQL SQL, so that we aren't locked out of any database features.
- Track migrations that have been applied to database in a table.

# Commands

Proposal of commands:

- `migrator init`
- `migrator migration new <name in kebab case>` creates a new migration with the provided name, picking an appropriate datetime.
- `migrator migration pin <migration>` pins the migration with the current components.
- `migrator migration build <migration> --pinned=<true|false>` builds the migration into the needed SQL.  `--pinned is required`.

# Thoughts

Think about this: can I version stored procedures in a clever way that involves automatically creating and referencing new version of stored procedures, but only when updated.  E.g., add_func_v1 calls other_func_v1.  We update add_func and now have add_func_v2, but other_func_v1 hasn't changed so add_func_v2 points to old.  Advantage: when we do an update, we're only updating a small number of stored procs.  Downside: very complex to work with without tooling.

Alternatively, put everything in app code by default, and send the whole procedure over as an anonymous function each call.

Another:
- For example, have your application setup a schema to contain its version-specific database components. The schema will contain an immutable application version, such as its commit hash, in its name. This allows a given version of the application to only use its own set of sprocs and views. On deploy, run the SQL scripts to create the sprocs and views for that version.

Have been recommended to  avoiding having views depend on other views.  And particularly, not having views depend on materialised views.
