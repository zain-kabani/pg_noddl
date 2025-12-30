--
-- pg_noddl regression tests
--

-- Setup
create extension pg_noddl;

--
-- Test: Basic functions exist and work
--
select pg_noddl_version();
select pg_noddl_is_enabled();
select pg_noddl_admin_user();

--
-- Test: Enable and disable
--
select pg_noddl_enable();
select pg_noddl_is_enabled();
select pg_noddl_disable();
select pg_noddl_is_enabled();

--
-- Test: DDL is blocked when enabled
--
drop table if exists test_table;
create table test_table (id int);

select pg_noddl_enable();

-- These should all fail
create table blocked_table (id int);
alter table test_table add column name text;
create index blocked_idx on test_table(id);
create type blocked_type as (x int, y int);
create function blocked_func() returns int as $$ select 1; $$ language sql;

select pg_noddl_disable();

--
-- Test: DML and safe operations work when enabled
--
select pg_noddl_enable();

-- DML should work
insert into test_table values (1);
update test_table set id = 2 where id = 1;
delete from test_table where id = 2;
select * from test_table;

-- Safe operations should work
set search_path to public;
show search_path;
begin; rollback;
vacuum test_table;
analyze test_table;

select pg_noddl_disable();

--
-- Test: Publication/subscription DDL is blocked for regular users
--
select pg_noddl_enable();

-- These should fail for regular users
create publication test_pub for table test_table;

select pg_noddl_disable();

--
-- Test: Admin user validation - non-existent user should fail
--
select pg_noddl_enable_with_admin('nonexistent_user_12345');

--
-- Test: Admin user bypass functionality
--
create role noddl_admin login createrole;
create role regular_user login;

-- Grant permissions needed for publication management
grant create on database postgres to noddl_admin;
alter table test_table owner to noddl_admin;

select pg_noddl_enable_with_admin('noddl_admin');
select pg_noddl_admin_user();

-- Current superuser should be blocked from DDL
create table should_fail (id int);

-- Admin should be able to do admin-bypassable DDL (role management)
set session authorization noddl_admin;
create role test_role_by_admin;
drop role test_role_by_admin;
reset session authorization;

-- Admin should be able to manage publications (for replication setup)
set session authorization noddl_admin;
create publication test_pub for table test_table;
alter publication test_pub drop table test_table;
reset session authorization;

-- Admin should still be blocked from schema DDL
set session authorization noddl_admin;
create table admin_blocked_table (id int);
reset session authorization;

-- Regular user should be blocked from role DDL
set session authorization regular_user;
create role regular_user_role;
reset session authorization;

-- Cleanup
select pg_noddl_disable();
drop publication test_pub;
alter table test_table owner to current_user;
revoke create on database postgres from noddl_admin;
drop role noddl_admin;
drop role regular_user;

--
-- Test: TRUNCATE works (replicated by logical replication)
--
insert into test_table values (1), (2), (3);
select pg_noddl_enable();
truncate test_table;
select * from test_table;
select pg_noddl_disable();

--
-- Cleanup
--
drop table test_table;
drop extension pg_noddl;
