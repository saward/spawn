---
title: Timestamps in Tests
description: Handling timestamps and other non-deterministic values in test output.
---

Spawn tests work by comparing actual output against expected output with a simple diff. This means any value that changes between runs – timestamps, auto-generated IDs, sequence values – will cause test failures even when the underlying logic is correct.

The fix is straightforward: keep non-deterministic values out of your test output.

## Timestamps

Instead of selecting a timestamp directly:

```sql
-- Bad: output changes every run
SELECT name, created_at FROM users;
```

Assert against a condition that produces a stable boolean:

```sql
-- Good: stable output
SELECT name, created_at > NOW() - INTERVAL '1 minute' AS recently_created
FROM users;
```

This verifies the timestamp is reasonable without including the actual value in the output.

## IDs

Auto-incrementing IDs and generated UUIDs will differ across runs if your test recreates the database. A few ways to handle this:

### Pre-generate IDs

Use a uuid generator to create IDs at render time, then insert with those known values:

```sql
{% set user_id = "301ca49b-1606-4960-a98b-afda7432d3bc" -%}

INSERT INTO users (id, email, name)
VALUES ({{ user_id }}, 'alice@example.com', 'Alice');

-- The ID is known and stable across runs
SELECT name FROM users WHERE id = {{ user_id }};
```

### Exclude IDs from output

If you only care about non-ID columns, just don't select the ID:

```sql
-- Bad: includes auto-generated ID
SELECT * FROM users;

-- Good: only select what you're testing
SELECT name, email FROM users;
```

### Use fixed IDs in test data

When you control the test data, insert with explicit IDs:

```sql
INSERT INTO users (id, email, name) VALUES (1, 'alice@test.com', 'Alice');

SELECT id, name FROM users WHERE id = 1;
```

This only works reliably when the test has full control over the data (e.g. using a fresh database copy via `WITH TEMPLATE`).

## Other non-deterministic values

The same principle applies to anything that varies between runs:

- **`random()`** -- avoid in test queries, or seed it with `setseed()`
- **Row ordering** -- add `ORDER BY` to any query where row order matters for the diff

## Summary

The rule of thumb: if a value can change between runs, either exclude it from your test output or replace it with a stable assertion.
