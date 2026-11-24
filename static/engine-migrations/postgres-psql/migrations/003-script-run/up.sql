BEGIN;

-- Audit trail for `spawn script run`. Append-only: a script may be run
-- any number of times, so unlike migration_history there is no migration row
-- to hang these off and no uniqueness to enforce.
CREATE TABLE IF NOT EXISTS {{variables.schema|escape_identifier}}.script_run (
    script_run_id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name TEXT NOT NULL,
    namespace TEXT NOT NULL DEFAULT 'default',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    status_id_status TEXT NOT NULL REFERENCES {{variables.schema|escape_identifier}}.status (status_id),
    -- Hash of the script's raw, unrendered source, never the executed SQL:
    -- the rendered output may contain resolved secret values.
    checksum BYTEA NOT NULL,
    pin_hash TEXT,
    execution_time INTERVAL NOT NULL
);

CREATE INDEX IF NOT EXISTS script_run_name_created_at_idx
    ON {{variables.schema|escape_identifier}}.script_run (name, created_at DESC);

COMMIT;
