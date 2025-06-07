-- {{ env }}
{% set dbname = "exampletest" %}
create database {{dbname}} with template spawn;
\c {{dbname}}
select '{{ env }}' as env;
select true as ok_working;
\c postgres
drop database {{dbname}};
