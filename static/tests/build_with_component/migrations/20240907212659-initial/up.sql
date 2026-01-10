BEGIN;
-- Created by {{ variables.name | safe }}
-- Environment: {{ env | safe }}

{% set myid = gen_uuid_v5("some seed") %}
-- uuid var: {{ myid | safe }}

{% include "util/add_func.sql" %}

COMMIT;
