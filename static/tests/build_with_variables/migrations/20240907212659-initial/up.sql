BEGIN;
-- Migration: {{ variables.migration_name | safe }}
-- Author: {{ variables.author | safe }}
-- Environment: {{ env | safe }}

CREATE TABLE {{ variables.table_name }} (
    id SERIAL PRIMARY KEY,
    name VARCHAR({{ variables.name_length }}) NOT NULL,
    active BOOLEAN DEFAULT {{ variables.default_active }}
);

COMMIT;
