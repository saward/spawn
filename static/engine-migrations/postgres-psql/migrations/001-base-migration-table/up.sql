CREATE SCHEMA IF NOT EXISTS {{schema}};

CREATE TABLE IF NOT EXISTS {{schema}}.migration (
    migration_id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name VARCHAR(256),
    namespace TEXT NOT NULL DEFAULT 'default',
    CONSTRAINT name_namespace_uq UNIQUE (name, namespace)
);

CREATE TABLE IF NOT EXISTS {{schema}}.activity(
    activity_id TEXT PRIMARY KEY CHECK (UPPER(activity_id) = activity_id)
);

CREATE TABLE IF NOT EXISTS {{schema}}.status(
    status_id TEXT PRIMARY KEY CHECK (UPPER(status_id) = status_id)
);

CREATE TABLE IF NOT EXISTS {{schema}}.migration_history (
    migration_history_id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    migration_id_migration BIGINT NOT NULL REFERENCES {{schema}}.migration (migration_id),
    activity_id_activity TEXT NOT NULL REFERENCES {{schema}}.activity (activity_id),
    created_by TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    description TEXT NOT NULL,
    status_note TEXT NOT NULL,
    status_id_status TEXT NOT NULL REFERENCES {{schema}}.status (status_id),
    checksum BYTEA NOT NULL,
    pin_hash TEXT,
    execution_time interval NOT NULL
);

INSERT INTO {{schema}}.activity (activity_id) VALUES
('APPLY'),
('REVERT')
ON CONFLICT DO NOTHING;

INSERT INTO {{schema}}.status (status_id) VALUES
('SUCCESS'),
('ATTEMPTED'),
('FAILURE')
ON CONFLICT DO NOTHING;
