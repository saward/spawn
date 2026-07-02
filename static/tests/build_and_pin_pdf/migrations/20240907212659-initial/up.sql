BEGIN;

SELECT {{ "small.bin"|read_file|base64_encode }};

COMMIT;
