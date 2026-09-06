CREATE TYPE attempt_scope AS ENUM ('login', 'share');

CREATE TABLE attempts (
    scope    attempt_scope NOT NULL,
    subject  TEXT          NOT NULL,
    failures INTEGER       NOT NULL CHECK (failures > 0),
    last_at  TIMESTAMPTZ   NOT NULL DEFAULT now(),

    PRIMARY KEY (scope, subject)
);

CREATE INDEX attempts_by_age ON attempts (last_at);
