ALTER TABLE nodes DROP CONSTRAINT nodes_name_check;

ALTER TABLE nodes ADD CONSTRAINT nodes_name_check CHECK (
    (parent_id IS NULL AND name = '')
    OR (parent_id IS NOT NULL AND name <> '' AND name !~ '/')
);
