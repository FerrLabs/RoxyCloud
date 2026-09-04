ALTER TABLE nodes ADD COLUMN trash_root_id UUID REFERENCES nodes (id) ON DELETE CASCADE;

WITH RECURSIVE batches AS (
    SELECT trashed.id, trashed.id AS root
    FROM nodes trashed
    LEFT JOIN nodes parent ON parent.id = trashed.parent_id
    WHERE trashed.deleted_at IS NOT NULL
      AND (parent.id IS NULL OR parent.deleted_at IS NULL)
    UNION ALL
    SELECT descendant.id, batches.root
    FROM nodes descendant
    JOIN batches ON descendant.parent_id = batches.id
    WHERE descendant.deleted_at IS NOT NULL
)
UPDATE nodes SET trash_root_id = batches.root FROM batches WHERE nodes.id = batches.id;

ALTER TABLE nodes ADD CONSTRAINT trashed_nodes_name_their_root CHECK (
    (deleted_at IS NULL) = (trash_root_id IS NULL)
);

CREATE INDEX nodes_trash_batch ON nodes (trash_root_id) WHERE deleted_at IS NOT NULL;
