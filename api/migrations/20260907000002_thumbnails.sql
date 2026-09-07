CREATE TABLE thumbnails (
    source_hash BYTEA       NOT NULL REFERENCES blobs (hash),
    edge        INTEGER     NOT NULL CHECK (edge > 0),
    blob_hash   BYTEA       NOT NULL REFERENCES blobs (hash),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (source_hash, edge)
);

CREATE INDEX thumbnails_by_blob ON thumbnails (blob_hash);
