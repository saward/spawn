# Spawn

### The Database Build System.

[![License](https://img.shields.io/badge/license-AGPL-blue.svg)](LICENSE)
[![Docs](https://img.shields.io/badge/docs-spawn.dev-green)](https://docs.spawn.dev)

**Stop treating your database like a script runner. Start treating it like a codebase.**

I like to lean heavily on the database. I don't like tools that abstract away the raw power of databases like PostgreSQL. Spawn is designed for developers who want to use the full breadth of modern database features: Functions, Views, Triggers, RLS -- while keeping the maintenance nightmares to a minimum.

Spawn introduces **Components**, **Compilation**, and **Reproducibility** to SQL migrations.

## Installing

[![Install Spawn](https://img.shields.io/badge/Get_Started-Install_Spawn-2ea44f?style=for-the-badge&logo=rocket&logoColor=white)](https://docs.spawn.dev/getting-started/install/)

Or simply:

```bash
# Install (macOS/Linux)
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/saward/spawn/releases/latest/download/spawn-db-installer.sh | sh
```

---

## The Philosophy

Standard migration tools (Flyway, dbmate) are great at running scripts, but bad at managing code. When you update a complex function, you have to copy-paste the code into a new file, which is cumbersome, makes your Git history messy, and makes code reviews more challenging.

**Spawn works differently:**

1.  **Edit in Place:** Keep your functions in `components/`. Edit them there. Get perfect Git diffs.
2.  **Pin in Time:** When you create a migration, Spawn **snapshots** your components in an efficient git-like storage, referenced per-migration via their `lock.toml`.
3.  **Compile to SQL:** Spawn compiles your templates and pinned components into standard SQL transactions.

> See it in action in the [Tutorial](https://docs.spawn.dev/getting-started/magic/).

---

## Key Features

### 📦 Component System (CAS)

Store reusable SQL snippets (views, functions, triggers) in a dedicated folder. When you create a migration, `spawn migration pin` creates a content-addressable snapshot of the entire tree.

- **Result:** Old migrations never break, because they point to the _snapshot_ of the function from 2 years ago, not the version in your folder today.

> Docs: [Tutorial: Components](https://docs.spawn.dev/getting-started/magic/) | [Templating](https://docs.spawn.dev/reference/templating/)

### 🧪 Integration Testing Framework

Spawn includes a native testing harness designed for SQL.

- **Macros:** Use [Minijinja](https://github.com/mitsuhiko/minijinja) macros to create reusable data factories (`{{ create_user('alice') }}`).
- **Ephemeral DBs:** Tests can run against temporary database copies (`WITH TEMPLATE`) for speed.
- **Diff-Based Assertions:** Tests pass if the output matches your `expected` file.

> Docs: [Tutorial: Testing](https://docs.spawn.dev/getting-started/magic/) | [Test Macros](https://docs.spawn.dev/recipes/test-macros/)

### 🚀 Zero-Abstractions

Spawn wraps `psql`. If you can do it in Postgres, you can do it in Spawn.

- No ORM limitations.
- No waiting for the tool to support a new Postgres feature.
- Full support for `\gset`, `\copy`, and other psql meta-commands.

### ☁️ Cloud Native

Connecting to production databases can be configured to use all your standard commands. You just need to provide it with a valid psql pipe.
Spawn supports **Provider Commands** -- configure it to use `gcloud`, `aws`, or `az` CLIs to resolve the connection or SSH tunnel automatically.

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

---

## A Quick Look

Here is how you handle a changing database function in Spawn without breaking history.

**1. Create a Component** (Your Source of Truth)

_`components/pricing/vat.sql`_

```sql
CREATE OR REPLACE FUNCTION calculate_vat(price numeric) RETURNS numeric AS $$
BEGIN
    RETURN price * 0.20; -- Current VAT rate
END;
$$ LANGUAGE plpgsql;
```

**2. Write a Migration** (Import, don't Copy)

_`migrations/20260101-add-vat/up.sql`_

```sql
BEGIN;
{% include 'pricing/vat.sql' %}
COMMIT;
```

**3. Pin It** (The Magic)

Run `spawn migration pin`. Spawn calculates the hash of `vat.sql` and locks this migration to that specific version.

```toml
# migrations/20260101-add-vat/lock.toml
pin = "a1b2c3d4..." # 🔒 This migration will ALWAYS use the 20% version.
```

**4. Evolve It**

Next year, when VAT changes to 22%, you simply update `components/pricing/vat.sql` and create a _new_ migration. The old migration remains locked to the old 20% version forever. **Zero copy-pasting.**

## Roadmap

Spawn is currently in **Public Beta**. The core features are stable and production-ready.

**Currently Supported:**

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

## Contributing

We welcome contributions! Please read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a PR. Note that this project requires signing a CLA.
