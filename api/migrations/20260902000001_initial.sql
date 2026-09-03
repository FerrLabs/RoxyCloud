CREATE TYPE node_kind AS ENUM ('directory', 'file');

CREATE TABLE blobs (
    hash         BYTEA PRIMARY KEY,
    size         BIGINT      NOT NULL CHECK (size >= 0),
    ref_count    BIGINT      NOT NULL DEFAULT 0 CHECK (ref_count >= 0),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    unreferenced_since TIMESTAMPTZ
);

CREATE INDEX blobs_sweepable ON blobs (unreferenced_since)
    WHERE ref_count = 0;

CREATE TABLE nodes (
    id         UUID PRIMARY KEY,
    owner_id   UUID        NOT NULL,
    parent_id  UUID        REFERENCES nodes (id) ON DELETE CASCADE,
    name       TEXT        NOT NULL CHECK (name <> '' AND name !~ '/'),
    kind       node_kind   NOT NULL,
    blob_hash  BYTEA       REFERENCES blobs (hash),
    size       BIGINT      NOT NULL DEFAULT 0 CHECK (size >= 0),
    etag       TEXT        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,

    CONSTRAINT files_carry_a_blob CHECK (
        (kind = 'file'      AND blob_hash IS NOT NULL) OR
        (kind = 'directory' AND blob_hash IS NULL AND size = 0)
    )
);

CREATE UNIQUE INDEX nodes_unique_name_per_parent
    ON nodes (parent_id, name)
    WHERE deleted_at IS NULL;

CREATE UNIQUE INDEX nodes_single_root_per_owner
    ON nodes (owner_id)
    WHERE parent_id IS NULL AND deleted_at IS NULL;

CREATE INDEX nodes_children ON nodes (parent_id) WHERE deleted_at IS NULL;
CREATE INDEX nodes_owner_trash ON nodes (owner_id, deleted_at) WHERE deleted_at IS NOT NULL;
CREATE INDEX nodes_by_blob ON nodes (blob_hash) WHERE blob_hash IS NOT NULL;

CREATE TABLE quotas (
    owner_id   UUID PRIMARY KEY,
    bytes_used BIGINT      NOT NULL DEFAULT 0 CHECK (bytes_used >= 0),
    bytes_max  BIGINT      NOT NULL CHECK (bytes_max > 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
