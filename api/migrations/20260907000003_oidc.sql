CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- The authorization request's state and its PKCE verifier, which must not leave the server: the
-- verifier is what proves the code being exchanged belongs to the browser that started the flow.
CREATE TABLE oidc_flows (
    state      TEXT PRIMARY KEY,
    verifier   TEXT        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX oidc_flows_by_expiry ON oidc_flows (expires_at);
