BEGIN;
-- Created by {{ variables.name }}
-- Environment: {{ env }}

{% set myid = gen_uuid_v5("some seed") %}
-- uuid var: {{ myid }}

{% include "util/add_func.sql" %}

COMMIT;
