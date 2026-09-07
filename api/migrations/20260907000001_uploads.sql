CREATE TABLE uploads (
    id         UUID PRIMARY KEY,
    owner_id   UUID        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    path       TEXT        NOT NULL CHECK (path <> ''),
    size       BIGINT      NOT NULL CHECK (size >= 0),
    received   BIGINT      NOT NULL DEFAULT 0 CHECK (received >= 0),
    staged     TEXT        NOT NULL,

    -- Claimed for the length of one write. Two requests at the same offset do not share a file
    -- cursor, so truncating to the offset is not enough to keep them from interleaving.
    writing_until TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,

    CONSTRAINT never_more_than_promised CHECK (received <= size)
);

CREATE INDEX uploads_of_owner ON uploads (owner_id);
CREATE INDEX uploads_by_expiry ON uploads (expires_at);
