---
title: Test Macros
description: Using Minijinja macros to create reusable test data factories.
---

Test macros let you define reusable data factories directly in SQL, making it easy to set up complex test scenarios.

## Basic macro

Define a macro that creates a user, to reduce boilerplate for SQL creation and provide defaults when not important.

```sql
{% macro create_user(email, name="Test User") %}
INSERT INTO users (email, name, created_at)
VALUES ({{ email }}, {{ name }}, NOW())
RETURNING id;
{% endmacro -%}

BEGIN;

-- Create test users
{{ create_user("alice@example.com", "Alice") }}
{{ create_user("bob@example.com") }}

ROLLBACK;
```

Which produces the following output:

```sql
BEGIN;

-- Create test users

INSERT INTO users (email, name, created_at)
VALUES ('alice@example.com', 'Alice', NOW())
RETURNING id;


INSERT INTO users (email, name, created_at)
VALUES ('bob@example.com', 'Test User', NOW())
RETURNING id;


ROLLBACK;
```

## Working with IDs

When tests need to insert related records, you need a way to reference IDs across statements. Here are three approaches.

### Generate IDs upfront

The simplest approach: generate IDs before inserting, so you always have them available.

```sql
{% macro create_user(id, email, name="Test User") %}
INSERT INTO users (id, email, name, created_at)
VALUES ({{ id }}, {{ email }}, {{ name }}, NOW());
{% endmacro %}

{% macro create_post(user_id, title) %}
INSERT INTO posts (user_id, title, created_at)
VALUES ({{ user_id }}, {{ title }}, NOW());
{% endmacro -%}

{% set user_1_id = gen_uuid_v4() %}

BEGIN;

{{ create_user(id=user_1_id, email="alice@example.com", name="Alice") }}
{{ create_post(user_id=user_1_id, title="First Post") }}
{{ create_post(user_id=user_1_id, title="Second Post") }}

ROLLBACK;
```

This works well when your table uses UUIDs as primary keys. The `gen_uuid_v4()` function generates the ID at template render time, so it can be reused across multiple statements.

### Capture IDs with `\gset`

Since Spawn runs SQL through `psql`, you can use [`\gset`](https://www.postgresql.org/docs/current/app-psql.html#APP-PSQL-META-COMMAND-GSET) to capture query results into psql variables:

```sql
{% macro create_user(email, name="Test User") %}
INSERT INTO users (email, name, created_at)
VALUES ({{ email }}, {{ name }}, NOW())
RETURNING id;
{% endmacro -%}

BEGIN;

-- Create user and capture the generated ID
SELECT id AS user_1_id FROM ({{ create_user("alice@example.com", "Alice") }}) AS t \gset

-- Use the captured ID in subsequent statements
INSERT INTO posts (user_id, title, created_at)
VALUES (:'user_1_id', 'First Post', NOW());

INSERT INTO posts (user_id, title, created_at)
VALUES (:'user_1_id', 'Second Post', NOW());

ROLLBACK;
```

The `\gset` meta-command stores each column of the result row as a psql variable. You then reference it with `:'variable_name'` syntax.

### Use RETURNING with a CTE

For a pure-SQL approach without psql features:

```sql
{% macro create_user_with_posts(email, name, posts) %}
WITH new_user AS (
  INSERT INTO users (email, name, created_at)
  VALUES ({{ email }}, {{ name }}, NOW())
  RETURNING id
)
INSERT INTO posts (user_id, title, created_at)
SELECT id, title, NOW()
FROM new_user, (VALUES {% for post in posts %}({{ post }}){% if not loop.last %}, {% endif %}{% endfor %}) AS t(title);
{% endmacro -%}

BEGIN;

{{ create_user_with_posts("alice@example.com", "Alice", ["First Post", "Second Post"]) }}

ROLLBACK;
```

This keeps everything in a single statement, but becomes harder to read with more complex relationships.

## Parameterized factories

Create factories with optional fields:

```sql
{% macro create_post(user_id, title, content="", published=false) %}
INSERT INTO posts (user_id, title, content, published, created_at)
VALUES (
  {{ user_id }},
  {{ title }},
  {{ content }},
  {{ published }},
  NOW()
)
RETURNING id;
{% endmacro %}
```

Usage:

```sql
-- Minimal
{{ create_post(1, "Draft Post") }}

-- With all options
{{ create_post(1, "Published Post", "Full content here", true) }}
```

## Bulk data generation

Use loops to generate test data at scale:

```sql
{% macro create_test_users(count) %}
{% for i in range(count) %}
INSERT INTO users (email, name, created_at)
VALUES ('user{{ i | safe }}@test.com', 'Test User {{ i | safe }}', NOW() - INTERVAL '{{ i | safe }} days');
{% endfor %}
{% endmacro %}

BEGIN;

-- Create 100 test users
{{ create_test_users(100) }}

-- Test query performance
EXPLAIN ANALYZE
SELECT * FROM users WHERE created_at > NOW() - INTERVAL '30 days';

ROLLBACK;
```

## Hierarchical data

Create related records in one macro:

```sql
{% macro create_organization_with_users(org_name, user_count) %}
WITH new_org AS (
  INSERT INTO organizations (name)
  VALUES ({{ org_name }})
  RETURNING id
)
{%- for i in range(user_count) %}
INSERT INTO users (organization_id, email, name)
SELECT id, '{{ org_name | lower | replace(" ", "-") | safe }}-user{{ i | safe }}@test.com', 'User {{ i | safe }}'
FROM new_org;
{%- endfor %}
{% endmacro %}

BEGIN;

{{ create_organization_with_users("Acme Corp", 5) }}
{{ create_organization_with_users("Globex Inc", 3) }}

-- Test
SELECT o.name, COUNT(u.id) as user_count
FROM organizations o
LEFT JOIN users u ON o.id = u.organization_id
GROUP BY o.id, o.name;

ROLLBACK;
```

## Shared macro library

Store commonly used macros in a component file:

**components/test_macros.sql:**

```sql
{% macro create_user(email, name="Test User") %}
INSERT INTO users (email, name, created_at)
VALUES ({{ email }}, {{ name }}, NOW())
RETURNING id;
{% endmacro %}

{% macro create_post(user_id, title) %}
INSERT INTO posts (user_id, title, created_at)
VALUES ({{ user_id }}, {{ title }}, NOW())
RETURNING id;
{% endmacro %}
```

**tests/user-posts/test.sql:**

```sql
{% from "test_macros.sql" import create_user, create_post %}

BEGIN;

-- Use shared macros
{{ create_user("alice@example.com", "Alice") }}
{{ create_post(1, "Alice's First Post") }}

-- Test
SELECT COUNT(*) FROM posts WHERE user_id = 1;

ROLLBACK;
```

## State management patterns

Set up and tear down test state:

```sql
{% macro setup_test_data() %}
-- Disable triggers during test setup
SET session_replication_role = 'replica';

-- Create test data
INSERT INTO users (id, email, name) VALUES
  (1, 'alice@test.com', 'Alice'),
  (2, 'bob@test.com', 'Bob');

-- Re-enable triggers
SET session_replication_role = 'origin';
{% endmacro %}

BEGIN;

{{ setup_test_data() }}

-- Run your test
SELECT * FROM users WHERE id = 1;

ROLLBACK;
```

## Learn more

- [Templating Reference](/reference/templating/)
- [Minijinja macro documentation](https://docs.rs/minijinja/latest/minijinja/syntax/index.html#macros)
