BEGIN;
{%- set json_data = "data.json"|read_file|to_string_lossy|parse_json %}
{%- set toml_data = "data.toml"|read_file|to_string_lossy|parse_toml %}
{%- set yaml_data = "data.yaml"|read_file|to_string_lossy|parse_yaml %}

-- from json
CREATE TABLE {{ json_data.table_name | escape_identifier }} (id SERIAL PRIMARY KEY);
SELECT * FROM {{ json_data.table_name | escape_identifier }} LIMIT {{ json_data.limit }};

-- from toml
SELECT * FROM {{ toml_data.table_name | escape_identifier }} LIMIT {{ toml_data.limit }};

-- from yaml
SELECT * FROM {{ yaml_data.table_name | escape_identifier }} LIMIT {{ yaml_data.limit }};

-- from json array
{%- set users = "users.json"|read_file|to_string_lossy|parse_json %}
{% for user in users -%}
INSERT INTO "users" (name, email) VALUES ({{ user.name }}, {{ user.email }});
{% endfor %}
COMMIT;
