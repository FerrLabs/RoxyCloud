CREATE TYPE attempt_scope AS ENUM ('login', 'share');

CREATE TABLE attempts (
    scope    attempt_scope NOT NULL,
    subject  TEXT          NOT NULL,
    failures INTEGER       NOT NULL CHECK (failures > 0),
    last_at  TIMESTAMPTZ   NOT NULL DEFAULT now(),

    -- A deadline that passes, so a lockout ends. Deciding on the count alone would mean the
    -- eleventh attempt shuts the subject out until a full day of silence.
    blocked_until TIMESTAMPTZ,

    PRIMARY KEY (scope, subject)
);

CREATE INDEX attempts_by_age ON attempts (last_at);
