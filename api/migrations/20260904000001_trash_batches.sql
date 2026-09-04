ALTER TABLE nodes ADD COLUMN trash_root_id UUID REFERENCES nodes (id) ON DELETE CASCADE;

UPDATE nodes SET trash_root_id = id WHERE deleted_at IS NOT NULL;

ALTER TABLE nodes ADD CONSTRAINT trashed_nodes_name_their_root CHECK (
    (deleted_at IS NULL) = (trash_root_id IS NULL)
);

CREATE INDEX nodes_trash_batch ON nodes (trash_root_id) WHERE deleted_at IS NOT NULL;
