---
title: Test Macros
description: Using Minijinja macros to create reusable test data factories.
---

Test macros let you define reusable data factories directly in SQL, making it easy to set up complex test scenarios.

## Basic macro

Define a macro that creates a user:

```sql
{% macro create_user(email, name="Test User") %}
INSERT INTO users (email, name, created_at)
VALUES ('{{ email }}', '{{ name }}', NOW())
RETURNING id;
{% endmacro %}
```

Use it in your test:

```sql
BEGIN;

-- Create test users
{{ create_user("alice@example.com", "Alice") }}
{{ create_user("bob@example.com", "Bob") }}

-- Run your test query
SELECT COUNT(*) FROM users;

ROLLBACK;
```

## Macros with IDs

Capture the returned ID for use in related records:

```sql
{% macro create_user(email, name) %}
WITH new_user AS (
  INSERT INTO users (email, name, created_at)
  VALUES ('{{ email }}', '{{ name }}', NOW())
  RETURNING id
)
SELECT id FROM new_user;
{% endmacro %}

BEGIN;

-- Create user and capture ID
DO $$
DECLARE
  user_id INTEGER;
BEGIN
  SELECT id INTO user_id FROM ({{ create_user("alice@example.com", "Alice") }}) AS u;
  
  -- Create posts for that user
  INSERT INTO posts (user_id, title, content)
  VALUES 
    (user_id, 'First Post', 'Hello world'),
    (user_id, 'Second Post', 'Another post');
END $$;

-- Test
SELECT u.name, COUNT(p.id) as post_count
FROM users u
LEFT JOIN posts p ON u.id = p.user_id
GROUP BY u.id, u.name;

ROLLBACK;
```

## Parameterized factories

Create factories with optional fields:

```sql
{% macro create_post(user_id, title, content="", published=false) %}
INSERT INTO posts (user_id, title, content, published, created_at)
VALUES (
  {{ user_id }}, 
  '{{ title }}', 
  '{{ content }}', 
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
VALUES ('user{{ i }}@test.com', 'Test User {{ i }}', NOW() - INTERVAL '{{ i }} days');
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
  VALUES ('{{ org_name }}')
  RETURNING id
)
{% for i in range(user_count) %}
INSERT INTO users (organization_id, email, name)
SELECT id, '{{ org_name | lower }}-user{{ i }}@test.com', 'User {{ i }}'
FROM new_org;
{% endfor %}
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

## Realistic test data

Use variables for more realistic data:

**test-data.json:**
```json
{
  "companies": [
    {"name": "Acme Corp", "industry": "Manufacturing"},
    {"name": "Globex Inc", "industry": "Technology"},
    {"name": "Initech", "industry": "Software"}
  ]
}
```

**Test:**
```sql
{% macro create_company(name, industry) %}
INSERT INTO companies (name, industry, created_at)
VALUES ('{{ name }}', '{{ industry }}', NOW())
RETURNING id;
{% endmacro %}

BEGIN;

{% for company in variables.companies %}
{{ create_company(company.name, company.industry) }}
{% endfor %}

-- Test
SELECT industry, COUNT(*) 
FROM companies 
GROUP BY industry;

ROLLBACK;
```

## Shared macro library

Store commonly used macros in a component file:

**components/test_macros.sql:**
```sql
{% macro create_user(email, name="Test User") %}
INSERT INTO users (email, name, created_at)
VALUES ('{{ email }}', '{{ name }}', NOW())
RETURNING id;
{% endmacro %}

{% macro create_post(user_id, title) %}
INSERT INTO posts (user_id, title, created_at)
VALUES ({{ user_id }}, '{{ title }}', NOW())
RETURNING id;
{% endmacro %}
```

**tests/user-posts/test.sql:**
```sql
{% include "test_macros.sql" %}

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
