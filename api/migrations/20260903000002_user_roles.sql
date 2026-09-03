CREATE TYPE user_role AS ENUM ('admin', 'member', 'reader');

ALTER TABLE users ADD COLUMN role user_role NOT NULL DEFAULT 'member';

UPDATE users SET role = 'admin' WHERE is_admin;

ALTER TABLE users DROP COLUMN is_admin;
