use serde::Serialize;
use sqlx::PgPool;

use crate::error::ApiError;
use roxycloud_core::grant::SHARED_WITH_ME;
use roxycloud_core::node::Node;
use roxycloud_core::user::User;

pub const DEFAULT_LIMIT: i64 = 50;
pub const MAX_LIMIT: i64 = 200;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Hit {
    #[sqlx(flatten)]
    #[serde(flatten)]
    pub node: Node,
    /// Where the match sits, relative to the account's root. A name on its own tells somebody they
    /// have a file called `notes.md` without telling them which of the four it is.
    pub path: String,
}

/// Substring match on the name, over the caller's live tree and the folders shared with them,
/// prefix matches first. A shared match is kept only when a mount the caller holds sits above it,
/// and its path climbs no further than that mount, so neither the owner's other files nor the
/// names above the shared folder can reach a page.
pub async fn by_name(
    pool: &PgPool,
    caller: &User,
    query: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<Hit>, ApiError> {
    let query = query.trim();
    if query.is_empty() {
        return Err(ApiError::WrongKind {
            expected: "search term",
        });
    }

    let escaped = like_escape(query);
    sqlx::query_as::<_, Hit>(
        r"WITH RECURSIVE mounts AS (
               SELECT grants.node_id, grants.mount_name, nodes.owner_id
               FROM grants
               JOIN nodes ON nodes.id = grants.node_id AND nodes.deleted_at IS NULL
               WHERE grants.grantee_email = $2
           ),
           matched AS (
               SELECT id, owner_id, parent_id, name, kind, blob_hash, size, etag,
                      created_at, updated_at, deleted_at
               FROM nodes
               WHERE (owner_id = $1 OR owner_id IN (SELECT owner_id FROM mounts))
                 AND deleted_at IS NULL
                 AND parent_id IS NOT NULL
                 AND name ILIKE '%' || $3 || '%' ESCAPE '\'
           ),
           climb AS (
               SELECT matched.id AS of, matched.id, matched.parent_id, 0 AS climbed
               FROM matched
               WHERE matched.owner_id <> $1
               UNION ALL
               SELECT climb.of, above.id, above.parent_id, climb.climbed + 1
               FROM climb
               JOIN nodes above ON above.id = climb.parent_id
               WHERE climb.id NOT IN (SELECT node_id FROM mounts)
           ) CYCLE id SET looped USING trail,
           entry AS (
               SELECT DISTINCT ON (climb.of) climb.of, climb.id AS mount_id
               FROM climb
               JOIN mounts ON mounts.node_id = climb.id
               ORDER BY climb.of, climb.climbed
           ),
           found AS (
               SELECT matched.*, entry.mount_id
               FROM matched
               LEFT JOIN entry ON entry.of = matched.id
               WHERE matched.owner_id = $1 OR entry.mount_id IS NOT NULL
               ORDER BY (matched.name ILIKE $3 || '%' ESCAPE '\') DESC, lower(matched.name),
                        matched.id
               LIMIT $4 OFFSET $5
           ),
           lineage AS (
               SELECT found.id AS of, found.id, found.parent_id, found.name, 0 AS climbed,
                      found.mount_id
               FROM found
               UNION ALL
               SELECT lineage.of, above.id, above.parent_id, above.name, lineage.climbed + 1,
                      lineage.mount_id
               FROM lineage
               JOIN nodes above ON above.id = lineage.parent_id
               WHERE above.parent_id IS NOT NULL
                 AND lineage.id IS DISTINCT FROM lineage.mount_id
                 AND (lineage.mount_id IS NOT NULL OR above.owner_id = $1)
           ) CYCLE id SET looped USING crumbs
           SELECT found.id, found.owner_id, found.parent_id,
                  CASE WHEN found.id = found.mount_id THEN mounts.mount_name ELSE found.name END
                      AS name,
                  found.kind, found.blob_hash, found.size, found.etag, found.created_at,
                  found.updated_at, found.deleted_at,
                  CASE WHEN found.mount_id IS NULL THEN
                      (SELECT string_agg(lineage.name, '/' ORDER BY lineage.climbed DESC)
                       FROM lineage WHERE lineage.of = found.id)
                  ELSE
                      concat_ws('/', $6::text, mounts.mount_name,
                          (SELECT string_agg(lineage.name, '/' ORDER BY lineage.climbed DESC)
                           FROM lineage
                           WHERE lineage.of = found.id AND lineage.id <> found.mount_id))
                  END AS path
           FROM found
           LEFT JOIN mounts ON mounts.node_id = found.mount_id
           ORDER BY (found.name ILIKE $3 || '%' ESCAPE '\') DESC, lower(found.name), found.id",
    )
    .bind(caller.id)
    .bind(&caller.email)
    .bind(&escaped)
    .bind(limit.clamp(1, MAX_LIMIT))
    .bind(offset.max(0))
    .bind(SHARED_WITH_ME)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

/// `%` and `_` are wildcards to `ILIKE`, so a search for `_` would match every name of any length
/// and a search for `%` would match the whole tree. What somebody typed is a literal.
fn like_escape(query: &str) -> String {
    let mut escaped = String::with_capacity(query.len());
    for character in query.chars() {
        if matches!(character, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards_are_escaped_into_literals() {
        assert_eq!(like_escape("report"), "report");
        assert_eq!(like_escape("100%"), r"100\%");
        assert_eq!(like_escape("a_b"), r"a\_b");
        assert_eq!(like_escape(r"back\slash"), r"back\\slash");
    }

    #[test]
    fn escaping_is_not_confused_by_what_it_just_added() {
        assert_eq!(
            like_escape(r"\%"),
            r"\\\%",
            "escaping the backslash and then re-escaping it would leave the percent bare"
        );
    }
}
