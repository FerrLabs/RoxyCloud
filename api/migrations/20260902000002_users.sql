CREATE TABLE users (
    id            UUID PRIMARY KEY,
    email         TEXT        NOT NULL,
    display_name  TEXT        NOT NULL,
    password_hash TEXT        NOT NULL,
    is_admin      BOOLEAN     NOT NULL DEFAULT false,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled_at   TIMESTAMPTZ
);

CREATE UNIQUE INDEX users_unique_email ON users (email);

ALTER TABLE nodes
    ADD CONSTRAINT nodes_owner_exists
    FOREIGN KEY (owner_id) REFERENCES users (id) ON DELETE CASCADE;

ALTER TABLE quotas
    ADD CONSTRAINT quotas_owner_exists
    FOREIGN KEY (owner_id) REFERENCES users (id) ON DELETE CASCADE;
