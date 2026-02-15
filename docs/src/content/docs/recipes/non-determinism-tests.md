---
title: Non-determinism in Tests
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

## Errors

Sometimes errors with psql will output information that includes timestamps. In some situations, using `\set VERBOSITY terse` will reduce the output from:

```
ERROR:  new row for relation "item" violates check constraint "item_quantity_on_hand_check"
DETAIL:  Failing row contains (1, Apple, 23.12, -1, 2026-02-15 10:04:32.199307+00, 2026-02-15 10:04:32.199307+00).
CONTEXT:  SQL statement "UPDATE item
    SET quantity_on_hand = quantity_on_hand + COALESCE(OLD.quantity, 0) - COALESCE(NEW.quantity, 0)
    WHERE item_id = COALESCE(NEW.item_id_item, OLD.item_id_item)"
PL/pgSQL function update_item_quantity_on_order_item_change() line 3 at SQL statement
SQL statement "INSERT INTO order_item (order_id_order, item_id_item, quantity, price_per_unit)
    SELECT v_order_id, items.item_id, items.quantity, i.price
    FROM UNNEST(p_items) AS items
    LEFT JOIN item i ON i.item_id = items.item_id"
PL/pgSQL function create_order(text,order_item_input[]) line 11 at SQL statement
```

To:

```
ERROR:  new row for relation "item" violates check constraint "item_quantity_on_hand_check"
```

## Other non-deterministic values

The same principle applies to anything that varies between runs:

- **`random()`** -- avoid in test queries, or seed it with `setseed()`
- **Row ordering** -- add `ORDER BY` to any query where row order matters for the diff

## Summary

The rule of thumb: if a value can change between runs, either exclude it from your test output or replace it with a stable assertion.
