CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- Trigram index so a substring match does not read the whole tree. Partial on the live nodes,
-- because the trash is never searched.
CREATE INDEX nodes_name_trigrams ON nodes USING GIN (name gin_trgm_ops)
    WHERE deleted_at IS NULL;
