-- Run with: spawn script run reset_demo_data
--
-- Scripts include from components/ exactly as migrations do, so the same
-- SQL can back both a schema change and an operational task.
BEGIN;

TRUNCATE users RESTART IDENTITY CASCADE;

{% include "util/add_func.sql" %}

COMMIT;
