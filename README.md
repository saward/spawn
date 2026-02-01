# Spawn

### The Database Build System.

[![License](https://img.shields.io/badge/license-AGPL-blue.svg)](LICENSE)
[![Docs](https://img.shields.io/badge/docs-spawn.dev-green)](https://docs.spawn.dev)

**Stop treating your database like a script runner. Start treating it like a codebase.**

I like to lean heavily on the database. I don't like tools that abstract away the raw power of databases like PostgreSQL. Spawn is designed for developers who want to use the full breadth of modern database features: Functions, Views, Triggers, RLS – while keeping the maintenance nightmares to a minimum.

Spawn introduces **Components**, **Compilation**, **Reproducibility**, and **Testing** to SQL migrations.

## Installing

[![Install Spawn](https://img.shields.io/badge/Get_Started-Install_Spawn-2ea44f?style=for-the-badge&logo=rocket&logoColor=white)](https://docs.spawn.dev/getting-started/install/)

Or simply:

```bash
# Install (macOS/Linux)
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/saward/spawn/releases/latest/download/spawn-db-installer.sh | sh
```

---

## The Philosophy

Standard migration tools (Flyway, dbmate) are great at running scripts, but bad at managing code. When you update a complex function, the solutions are usually one of these:

- Create a new migration, copy the old view/function into the new, and edit in the new.
- Repeatable migrations, which break migrations when running through them from the beginning.
- Complex solutions like with Sqitch, where a copy of your original migration is made, old scripts updated to point at old, and you edit old as the new.

These solutions can be cumbersome, make tracking changes over time and reviewing PRs challenging, and can break older migrations when running on a fresh database.

**Spawn works differently:**

1.  **Edit in Place:** Keep your functions in `components/`. Edit them there. Get perfect Git diffs.
2.  **Pin in Time:** When you create a migration, Spawn **snapshots** your components in an efficient git-like storage, referenced per-migration via their `lock.toml`.
3.  **Compile to SQL:** Spawn compiles your templates and pinned components into standard SQL transactions.

Old migrations work exactly as they did the first time they were created.

> See it in action in the [Tutorial](https://docs.spawn.dev/getting-started/magic/).

---

## A Quick Look: Migrations

**1. Setup**

Get a full Postgres environment running in a few seconds:

```bash
spawn init --docker && docker compose up -d
```

**2. Define & Pin**

Create a reusable component (your source of truth) and a migration.

_`spawn/components/users/name.sql`_:

```sql
CREATE OR REPLACE FUNCTION get_name(first text, last text) RETURNS text AS $$
BEGIN
    RETURN first || ' ' || last; -- V1 Logic
END;
$$ LANGUAGE plpgsql;
```

_`spawn/migrations/20260101-init/up.sql`_:

```sql
BEGIN;
CREATE TABLE users (id serial, first text, last text);
{% include 'users/name.sql' %} -- Include the component
COMMIT;
```

Run `spawn migration pin`. Spawn snapshots the V1 components and references them in a lockfile.

```toml
# spawn/migrations/20260101-init/lock.toml
pin = "a1b2c3d4..." # 🔒 Locked to V1 forever
```

**3. Evolve**

Months later, you update the **same file** to change the business logic around displaying names, and create a new migration.

_`spawn/components/users/name.sql`_ (Edited in place):

```sql
...
    RETURN first || ' ' || substring(last, 1, 1); -- V2 Logic
...
```

_`spawn/migrations/20260601-update/up.sql`_ (New Migration):

```sql
BEGIN;
-- Re-import the SAME component file, which now contains V2 logic
{% include 'users/name.sql' %}
COMMIT;
```

Pin the new migration:

```bash
spawn migration pin 20260601-update
```

**4. The Magic**

You changed the source code, but **you didn't break history.**
Prove it by building both migrations:

```bash
spawn migration build 20260101-init --pinned
```

```sql
-- Migration 1 (Built from Snapshot)
CREATE OR REPLACE FUNCTION get_name...
    RETURN first || ' ' || last; -- ✅ Still V1
```

```bash
spawn migration build 20260601-update --pinned
```

```sql
-- Migration 2 (Built from Snapshot)
CREATE OR REPLACE FUNCTION get_name...
    RETURN first || ' ' || substring(last, 1, 1); -- ✅ Updates to V2
```

**Zero copy-pasting. Zero broken dependencies.**

> Full tutorial with testing, templating, and more: [docs.spawn.dev/getting-started/magic](https://docs.spawn.dev/getting-started/magic/)

## A Quick Look: Regression Tests

**1. Write the Test**

Use plain SQL to write tests, and run them in a transaction or in a copy of the database via `WITH TEMPLATE`.

_`spawn/tests/users/test.sql`_

```sql
-- 1. Spin up a throwaway copy of your schema
CREATE DATABASE test_users WITH TEMPLATE postgres;
\c test_users

-- 2. Run scenarios
SELECT get_name('John', 'Doe'); -- Expecting full name

-- 3. Cleanup
\c postgres
DROP DATABASE test_users;
```

**2. Capture the Baseline**
Run the test and save the output as the "Source of Truth."

```bash
spawn test expect users
```

_`spawn/tests/users/expected`_

```text
 get_name
----------
 John Doe
(1 row)
```

**3. Catch Regressions (CI/CD)**
Later, you apply the V2 update (abbreviated last name), but the test still expects the full name. `spawn test compare` catches the behavioral change immediately.

```bash
spawn test compare users
```

```diff
[FAIL] users
--- Diff ---
   get_name
 ----------
-   John Doe
+   John D
 (1 row)

Error: ! Differences found in one or more tests
```

**No manual assertions. Run in GitHub Actions using the [Spawn Action](https://docs.spawn.dev/reference/ci-cd/).**

---

## Key Features

### 📦 Component System (CAS)

Store reusable SQL snippets (views, functions, triggers) in a dedicated folder. When you create a migration, `spawn migration pin` creates a content-addressable snapshot of the entire tree.

- **Result:** Old migrations never break, because they point to the _snapshot_ of the function from 2 years ago, not the version in your folder today.

> Docs: [Tutorial: Components](https://docs.spawn.dev/getting-started/magic/) | [Templating](https://docs.spawn.dev/reference/templating/)

### 🧪 Integration Testing Framework

Spawn includes a native testing harness designed for SQL.

- **Macros:** Use [Minijinja](https://github.com/mitsuhiko/minijinja) macros to create reusable data factories (`{{ create_user('alice') }}`).
- **Ephemeral Tests:** Tests can run against temporary database copies (`WITH TEMPLATE`) for speed, or within transactionsi when possible.
- **Diff-Based Assertions:** Tests pass if the output matches your `expected` file.

> Docs: [Tutorial: Testing](https://docs.spawn.dev/getting-started/magic/) | [Test Macros](https://docs.spawn.dev/recipes/test-macros/)

### 🚀 Zero-Abstractions

Spawn wraps `psql`. If you can do it in Postgres, you can do it in Spawn.

- No ORM limitations.
- No waiting for the tool to support a new Postgres feature.
- Full support for `\gset`, `\copy`, and other psql meta-commands.

### ☁️ Cloud Native

Connecting to production databases can be configured to use all your standard commands. You just need to provide it with a valid psql pipe.
Spawn supports **Provider Commands** – configure it to use `gcloud`, `aws`, or `az` CLIs to resolve the connection or SSH tunnel automatically.

```toml
# spawn.toml
[databases.prod]
command = {
    kind = "provider",
    provider = ["gcloud", "compute", "ssh", "--dry-run", ...],
    append = ["psql", ...]
}
```

> Docs: [Manage Databases](https://docs.spawn.dev/guides/manage-databases/) | [Configuration](https://docs.spawn.dev/reference/config/)

## Roadmap

Spawn is currently in **Public Beta**. It is fully functional and has test suites to help prevent regressions, but should be considered experimental software. We recommend testing thoroughly before adopting it for critical production workloads.

**Currently Supported:**

- ✅ PostgreSQL via psql support
- ✅ Core Migration Management (Init, New, Apply)
- ✅ Component Pinning & CAS
- ✅ Minijinja Templating
- ✅ Testing Framework (Run, Expect, Compare)
- ✅ Database Tracking & Advisory Locks
- ✅ [CI/CD Integration](https://docs.spawn.dev/reference/ci-cd/)

**What's Next:**

- 🔄 **Rollback Support:** Optional down scripts for reversible migrations.
- 🔄 **Additional Engines:** Native PostgreSQL driver, MySQL, and more.
- 🔄 **Multi-Tenancy:** First-class support for schema-per-tenant migrations.
- 🔄 **Drift Detection:** Compare expected vs actual database state.
- 🔄 **External Data Sources:** Better support for data from files, URLs, and scripts in templates.
- 🔄 **Plugin System:** Custom extensions for engines, data sources, and workflows.

_(See [Roadmap](https://docs.spawn.dev/reference/roadmap) for detailed tracking)_

---

## Documentation

Full documentation, recipes, and configuration guides are available at:

### [👉 docs.spawn.dev](https://docs.spawn.dev)

## Telemetry

Spawn collects anonymous usage data, to help us improve Spawn. Set `"telemetry = false"` in `spawn.toml` or use `DO_NOT_TRACK=1` to opt-out.

## Contributing

Please read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a PR. Note that this project requires signing a CLA.
