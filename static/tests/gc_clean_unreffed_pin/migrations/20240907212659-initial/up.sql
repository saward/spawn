BEGIN;

{% set myid = gen_uuid_v5("some seed") %}

COMMIT;
