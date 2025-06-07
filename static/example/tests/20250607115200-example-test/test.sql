-- {{ env }}
{% set dbname = "exampletest" %}
create database {{dbname}} with template spawn;
\c {{dbname}}
sel;
select '{{ env }}' as env;
select true as ok_working;
\c postgres
drop database {{dbname}};
