CREATE TABLE versions (
    id         UUID PRIMARY KEY,
    node_id    UUID        NOT NULL REFERENCES nodes (id) ON DELETE CASCADE,
    blob_hash  BYTEA       NOT NULL REFERENCES blobs (hash),
    size       BIGINT      NOT NULL CHECK (size >= 0),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX versions_by_node ON versions (node_id, id DESC);
