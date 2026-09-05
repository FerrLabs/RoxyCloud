CREATE TABLE locks (
    token      TEXT PRIMARY KEY,
    node_id    UUID        NOT NULL REFERENCES nodes (id) ON DELETE CASCADE,
    owner_id   UUID        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    holder     TEXT,
    deep       BOOLEAN     NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);

-- Only exclusive write locks exist, so a node holds at most one.
CREATE UNIQUE INDEX locks_one_per_node ON locks (node_id);
CREATE INDEX locks_by_expiry ON locks (expires_at);
