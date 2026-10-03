BEGIN;

INSERT INTO {{variables.schema|escape_identifier}}.activity (activity_id) VALUES
('DOWN')
ON CONFLICT DO NOTHING;

-- supersedes it.
UPDATE {{variables.schema|escape_identifier}}.migration_history
SET activity_id_activity = 'DOWN'
WHERE activity_id_activity = 'REVERT';

DELETE FROM {{variables.schema|escape_identifier}}.activity
WHERE activity_id = 'REVERT';

COMMIT;
