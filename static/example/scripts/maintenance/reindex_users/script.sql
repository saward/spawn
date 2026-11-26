-- Run with: spawn script run maintenance/reindex_users
--
-- No BEGIN;/COMMIT; on purpose: CREATE INDEX CONCURRENTLY cannot run inside
-- a transaction block, and a script owns its own transactions.
REINDEX INDEX CONCURRENTLY users_pkey;
