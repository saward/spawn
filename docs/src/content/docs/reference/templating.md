---
title: Templating
description: SQL templating with Minijinja in Spawn.
---

Spawn uses [Minijinja](https://docs.rs/minijinja/latest/minijinja/syntax/index.html) to render migration and test templates. This allows you to generate dynamic SQL based on variables, includes, and logic.

## Template files

Templates are used in:

- Migration files (`migrations/*/up.sql`)
- Component files (`components/**/*.sql`)
- Test files (`tests/*/test.sql`)

All template syntax is processed before SQL execution.

## Built-in variables

### `env`

The environment from the database config (e.g., `"dev"`, `"prod"`).

```sql
{% if env == "dev" %}
-- Insert test data only in dev
INSERT INTO users (email) VALUES ('test@example.com');
{% endif %}
```

### `variables`

Custom variables loaded from a JSON/TOML/YAML file via `--variables` or configured in `spawn.toml`.

**variables.json:**

```json
{
  "table_name": "users",
  "admin_email": "admin@example.com"
}
```

**Migration:**

```sql
CREATE TABLE {{ variables.table_name }} (
  id SERIAL PRIMARY KEY,
  email TEXT NOT NULL
);

INSERT INTO {{ variables.table_name }} (email)
VALUES ('{{ variables.admin_email }}');
```

## Including components

Use `{% include %}` to insert reusable SQL from the `components/` directory:

```sql
BEGIN;

{% include "functions/calculate_fee.sql" %}
{% include "views/active_users.sql" %}

COMMIT;
```

Component paths are relative to `components/`. The `.sql` extension is required.

## Control flow

### Conditionals

```sql
{% if env == "dev" %}
SET statement_timeout = 0;
{% else %}
SET statement_timeout = '30s';
{% endif %}
```

```sql
{% if variables.enable_feature %}
ALTER TABLE users ADD COLUMN feature_flag BOOLEAN DEFAULT true;
{% endif %}
```

### Loops

Iterate over arrays in variables:

**variables.json:**

```json
{
  "tenants": ["acme", "globex", "initech"]
}
```

**Migration:**

```sql
{% for tenant in variables.tenants %}
CREATE SCHEMA {{ tenant }};
CREATE TABLE {{ tenant }}.users (
  id SERIAL PRIMARY KEY,
  name TEXT
);
{% endfor %}
```

## Filters

Minijinja provides filters for transforming values:

```sql
-- Upper case
INSERT INTO logs (message) VALUES ('{{ variables.message | upper }}');

-- Default value if undefined
SELECT * FROM {{ variables.table | default(value="users") }};

-- Length
{% if variables.items | length > 0 %}
-- Process items
{% endif %}
```

See [Minijinja filters documentation](https://docs.rs/minijinja/latest/minijinja/filters/index.html) for the complete list.

## SQL-safe output

### Automatic escaping

Spawn automatically escapes string values in template output to prevent SQL injection:

```sql
-- Safe: automatically quoted and escaped
INSERT INTO users (name) VALUES ('{{ variables.user_name }}');
```

If `variables.user_name` is `O'Reilly`, the output is:

```sql
INSERT INTO users (name) VALUES ('O''Reilly');
```

### Identifiers

For table/column names (identifiers), use the `ident` filter:

```sql
-- Correct: identifier quoting
CREATE TABLE {{ variables.table_name | ident }} (
  id SERIAL PRIMARY KEY
);
```

If `variables.table_name` is `user-data`, the output is:

```sql
CREATE TABLE "user-data" (
  id SERIAL PRIMARY KEY
);
```

### Raw SQL

To bypass escaping (when you're generating SQL fragments), use the `safe` filter:

```sql
{% set conditions = "status = 'active' AND created_at > NOW() - INTERVAL '1 day'" %}
SELECT * FROM users WHERE {{ conditions | safe }};
```

**Warning:** Only use `safe` with trusted input. Never with user-provided data.

## Macros

Define reusable template functions:

```sql
{% macro create_audit_columns() %}
  created_at TIMESTAMPTZ DEFAULT NOW(),
  updated_at TIMESTAMPTZ DEFAULT NOW(),
  created_by TEXT,
  updated_by TEXT
{% endmacro %}

CREATE TABLE users (
  id SERIAL PRIMARY KEY,
  email TEXT NOT NULL,
  {{ create_audit_columns() }}
);

CREATE TABLE posts (
  id SERIAL PRIMARY KEY,
  title TEXT NOT NULL,
  {{ create_audit_columns() }}
);
```

See the [Test Macros recipe](/recipes/test-macros/) for practical examples.

## Comments

Template comments are removed from output:

```sql
{# This comment won't appear in the final SQL #}
SELECT * FROM users;
```

Use SQL comments for documentation that should remain:

```sql
-- This comment will appear in the final SQL
SELECT * FROM users;
```

## Whitespace control

Add `-` to strip whitespace:

```sql
{% for i in range(3) -%}
  SELECT {{ i }};
{% endfor %}
```

Output:

```sql
SELECT 0;
SELECT 1;
SELECT 2;
```

Without `-`, blank lines would appear between statements.

## Learn more

- [Minijinja documentation](https://docs.rs/minijinja/)
- [Minijinja template syntax](https://docs.rs/minijinja/latest/minijinja/syntax/index.html)
- [Test Macros recipe](/recipes/test-macros/)
