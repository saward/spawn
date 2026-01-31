---
title: Templating
description: SQL templating with Minijinja in Spawn.
---

Spawn uses [Minijinja](https://docs.rs/minijinja/latest/minijinja/syntax/index.html) to render migration and test templates. This allows you to generate dynamic SQL based on variables, includes, and logic.

:::tip[Full template syntax reference]
This page provides a very brief overview of templating syntax, along with a description of Spawn-specific templating features. For the complete template syntax including all expressions, operators, and built-in functions, see the [Minijinja template syntax documentation](https://docs.rs/minijinja/latest/minijinja/syntax/index.html).
:::

## Template files

Templates are used in:

- Migration files (`migrations/*/up.sql`)
- Component files (`components/**/*.sql`)
- Test files (`tests/*/test.sql`)

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

Component paths are relative to `components/`. The full file name including extension is required.

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

## SQL escaping and security

Spawn adds a security layer on top of Minijinja that automatically escapes all template output for SQL safety. This reduces the risk of SQL injection attacks by escaping provided template values by default. For the PostgreSQL psql engine, this means escaping variables as literals by default.

### How it works

When you use `{{ }}` to output a value, Spawn:

1. Detects the value type (string, number, boolean, etc.)
2. Applies PostgreSQL escaping rules appropriate for that type
3. Wraps strings in single quotes with proper escaping

This happens **automatically** for all template output, unlike plain Minijinja where you control escaping.

### Automatic literal escaping

By default, all values are escaped as **SQL literals** (values):

```sql
-- Automatically escaped and quoted
INSERT INTO users (name, age) VALUES ({{ user_name }}, {{ user_age }});
```

If `user_name` is `O'Reilly` and `user_age` is `42`:

```sql
INSERT INTO users (name, age) VALUES ('O''Reilly', 42);
```

Notice the string is automatically wrapped in single quotes and the embedded quote is doubled to prevent breaking out of the string literal. Numbers are output without quotes.

**SQL injection attempt is safely escaped:**

```sql
-- Input: user_name = "'; DROP TABLE users; --"
INSERT INTO users (name) VALUES ({{ user_name }});

-- Output (safe):
INSERT INTO users (name) VALUES ('''; DROP TABLE users; --');
```

### Identifier escaping

When you need to use a variable as a **table or column name** (identifier), use the `escape_identifier` filter:

```sql
-- Variable used as an identifier
SELECT * FROM my_schema.{{ table_name | escape_identifier }} my_table;

CREATE TABLE {{ schema_name | escape_identifier }}.{{ table_name | escape_identifier }} (
  id SERIAL PRIMARY KEY
);
```

If `table_name` is `user-data`:

```sql
SELECT * FROM my_schema."user-data" my_table;
```

The value is wrapped in double quotes and any embedded quotes are escaped.

**When to use `escape_identifier`:**

- Table names
- Column names
- Schema names
- View names
- Function names

**When NOT to use it:**

- String values in `WHERE` clauses (use default escaping)
- Numbers, booleans (use default escaping)
- Complete SQL expressions (use `safe` filter)

### Bypassing escaping with `safe`

To output raw SQL without any escaping (for trusted SQL fragments), use the `safe` filter:

```sql
{% set conditions = "status = 'active' AND created_at > NOW() - INTERVAL '1 day'" %}
SELECT * FROM users WHERE {{ conditions | safe }};
```

:::danger[Be careful with `safe`]
Use of `safe` may make it easier for untrusted SQL to make its way into your database. We recommend only using `safe` in the following ways:

- Hard-coded SQL fragments you write yourself
- SQL generated by other trusted parts of your templates
- **Never** with user input or external data
  :::

### Type-specific escaping

Spawn's auto-escaper handles different types appropriately:

```sql
-- String → single-quoted literal
{{ "hello" }}              -- Output: 'hello'

-- Number → unquoted
{{ 42 }}                   -- Output: 42
{{ 3.14 }}                 -- Output: 3.14

-- Boolean → PostgreSQL boolean
{{ true }}                 -- Output: TRUE
{{ false }}                -- Output: FALSE

-- null/undefined → NULL
{{ none }}                 -- Output: NULL
{{ undefined_var }}        -- Output: NULL

-- Array → PostgreSQL array literal
{{ [1, 2, 3] }}            -- Output: ARRAY[1, 2, 3]
{{ ["a", "b"] }}           -- Output: ARRAY['a', 'b']
```

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
