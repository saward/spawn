PostgreSQL migration management tool.

I like to lean heavily on the database.  That means I don't like to use tools that get in the way of that, stopping me from doing what I want.  That being said, working with the database could benefit from better tooling.

There are plenty of other migration solutions out there, but this one's mine.  The main alternative I've considered is sqitch, and while it seems to work pretty well, I've recently been looking at how to handle multiple tenants in a PostgreSQL database with a schema-per-tenant setup.  That means that I want to run the migrations per schema.  Unfortunately, sometimes I find that I need to mention specific names of the schemas or users, and when using psql's variables to substitute those in with sqitch, I hit unexpected limitations.

Here are some of my design goals with migrator:

- Handle history of functions/stored procs, so we can see proper history.
- Ability to write custom hand-crafted migrations.
- Rollback potentially in case of breaking change.
- Easy to spin up new tenant schema.
- Easy to migrate each tenant schema.
- Supporting migrations from multiple folders.  E.g., if a separate project provided some of your migrations, then you can apply migrations from both folders.
- Plain SQL mostly, or rather generates plain SQL that can be modified.
- Possibly support tests, thought that may be out of scope of this tool.
- Variables supported, for substitution, as well as matrices to generate migrations for a bunch of sites.  Or maybe we never generate and store the files, since there are many tenants, and instead run them against each schema somehow without generating stored/saved files for each schema.
- Keep track of which migrations have been applied, so that when targeting a schema it will check which need to be applied and then apply all.
- Watch a particular function or view, and re-apply automatically upon file change, for local testing.
 - Support a jinja template watch, where if the rendered jinja template changes it gets re-applied.  Useful in cases where we're updating views that depend on each other, and want to automatically recreate all those views as we edit files.
- Stretch goals:
 - Some clever way to watch changes in the view/function folder, and automatically update.  Functions are easier, but views will fail when columns change or they have dependencies.
  - I've tried having a schema dedicated to these things that are easy to throw away and rectrate, but the two problems are (a) it can get slow when there's more, making it hard to do in transactoin, and (b) I suspect we'll hit cases where can't be fully done inside transaction or rolled back.
 - Handle migration of views properly (e.g., when they depend on each other).
 - Reverting.  For now, likely this will assume you're performing DDL in a transaction in most cases.  Later, want to support something more official, particularly for cases where transactions are not possible or feasible.
 - Flatten schema.  E.g., deploy to local db with unique random values for variables (e.g., schema and user names), export again, and replace all references to the unique schema name with template variables again.

# Design

We have four primary folders:

1. `<base>/migrator/components`.  This contains standalone SQL snippets that can be modified and reused.  These are minijinja templates, but they could be plain SQL.  Typically, this is for DDL changes that don't affect data.  E.g., views or stored procedures.  The goal is to have proper change tracking for these, so that we can look at the history in git for that file and see how it has changed over time.
2. `<base>/migrator/templates`.  This folder contains migration templates.  E.g., `20240802030220-support-roles.sql.jinja`.  These are minijinja templates, designed to produce plain SQL migration scripts.  In these templates, you can import components.
3. `<base>/migrator/migrations`.  This contains the generated migration file based on the template and components.  E.g., `20240802030220-support-roles.sql`.  These can then be applied to the database.
4. `<base>/migrator/archive`.  Contains a copy of components used in a migration script, as they were at that time.  This is a reference, but also serves as a way to reuse at a later time if an older migration script needs to be updated for some reason, so it can use the version of the component as it was at that time.


Design goals:

- When updating functions or (some) views, you edit the file in the components folder for it in place, and reference that component in your new migration script.  This ensures that git diffs will show what's changed in the function, rather than being a fresh copy.
- A record of the components used in a migration script are kept as they were at that time.  This helps in case there is a need to return to an earlier migration script and update it for whatever reason.  The version of the component at that time can be used, instead of the current version.
- Support for variables, so that things such as schema names can be configurable (e.g., generating migrations for multiple tenancies in a schema-per-tenant setup).
- Support plain PostgreSQL SQL, so that we aren't locked out of any database features.
- Track migrations that have been applied to database in a table.
