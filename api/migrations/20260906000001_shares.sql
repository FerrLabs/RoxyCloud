CREATE TABLE shares (
    id           UUID PRIMARY KEY,
    node_id      UUID        NOT NULL REFERENCES nodes (id) ON DELETE CASCADE,
    created_by   UUID        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash   TEXT        NOT NULL UNIQUE,
    password_hash TEXT,
    expires_at   TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    revoked_at   TIMESTAMPTZ
);

CREATE INDEX shares_of_user ON shares (created_by, created_at DESC) WHERE revoked_at IS NULL;
CREATE INDEX shares_of_node ON shares (node_id) WHERE revoked_at IS NULL;
