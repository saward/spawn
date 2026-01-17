\set QUIET off
{% set dbname = "testclitest" %}
create database {{dbname|escape_identifier}} with template spawn;
\c {{dbname|escape_identifier}}
select 1 + 1 as result;
select 'hello' as greeting;
\c postgres
drop database {{dbname|escape_identifier}};
