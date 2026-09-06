CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE EXTENSION IF NOT EXISTS btree_gin;

-- Trigram index so a substring match does not read the whole tree. Partial on the live nodes,
-- because the trash is never searched, and carrying the owner so that one account's search does
-- not gather the trigram matches belonging to every other account.
CREATE INDEX nodes_name_trigrams ON nodes USING GIN (owner_id, name gin_trgm_ops)
    WHERE deleted_at IS NULL;
