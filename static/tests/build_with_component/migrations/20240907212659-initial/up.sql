BEGIN;
-- Created by {{ variables.name | default("unknown") | safe }}
-- Environment: {{ env | safe }}

{% set myid = gen_uuid_v5("some seed") %}
-- uuid var: {{ myid | safe }}

{% include "util/add_func.sql" %}

COMMIT;
