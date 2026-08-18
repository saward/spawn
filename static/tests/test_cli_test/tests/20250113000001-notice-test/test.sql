\set QUIET off
{% set dbname = "testclitestnotice" %}
create database {{dbname|escape_identifier}} with template spawn;
\c {{dbname|escape_identifier}}
drop table if exists nonexistent_table_xyz;
select 1 as ok;
\c postgres
drop database {{dbname|escape_identifier}};
