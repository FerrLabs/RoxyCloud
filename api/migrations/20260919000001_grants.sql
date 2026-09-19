CREATE TYPE grant_access AS ENUM ('read', 'write');

CREATE TABLE grants (
    id            UUID PRIMARY KEY,
    node_id       UUID         NOT NULL REFERENCES nodes (id) ON DELETE CASCADE,
    grantee_email TEXT         NOT NULL,
    access        grant_access NOT NULL,
    mount_name    TEXT         NOT NULL CHECK (mount_name <> '' AND mount_name !~ '/'),
    created_by    UUID         NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at    TIMESTAMPTZ  NOT NULL DEFAULT now(),
    CONSTRAINT grants_once_per_address UNIQUE (node_id, grantee_email),
    CONSTRAINT grants_mount_names UNIQUE (grantee_email, mount_name)
);

UPDATE nodes
SET name = name || ' (' || left(id::text, 8) || ')'
WHERE name = 'Shared with me'
  AND parent_id IN (SELECT id FROM nodes WHERE parent_id IS NULL);
