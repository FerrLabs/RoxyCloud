use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::ApiError;
use roxycloud_core::node::Node;

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

/// Substring match on the name, over one account's live tree, prefix matches first. The owner is a
/// bound parameter rather than a filter applied to a wider result, so there is no ordering of the
/// clauses that would let another account's node through.
pub async fn by_name(
    pool: &PgPool,
    owner_id: Uuid,
    query: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<Hit>, ApiError> {
    let query = query.trim();
    if query.is_empty() {
        return Err(ApiError::WrongKind {
            expected: "something to search for",
        });
    }

    let escaped = like_escape(query);
    sqlx::query_as::<_, Hit>(
        r"WITH RECURSIVE found AS (
               SELECT id, owner_id, parent_id, name, kind, blob_hash, size, etag,
                      created_at, updated_at, deleted_at
               FROM nodes
               WHERE owner_id = $1
                 AND deleted_at IS NULL
                 AND parent_id IS NOT NULL
                 AND name ILIKE '%' || $2 || '%' ESCAPE '\'
               ORDER BY (name ILIKE $2 || '%' ESCAPE '\') DESC, lower(name), id
               LIMIT $3 OFFSET $4
           ),
           lineage AS (
               SELECT found.id AS of, found.id, found.parent_id, found.name, 0 AS climbed
               FROM found
               UNION ALL
               SELECT lineage.of, above.id, above.parent_id, above.name, lineage.climbed + 1
               FROM lineage
               JOIN nodes above ON above.id = lineage.parent_id
               WHERE above.parent_id IS NOT NULL
           ) CYCLE id SET looped USING crumbs
           SELECT found.id, found.owner_id, found.parent_id, found.name, found.kind,
                  found.blob_hash, found.size, found.etag, found.created_at, found.updated_at,
                  found.deleted_at,
                  (SELECT string_agg(lineage.name, '/' ORDER BY lineage.climbed DESC)
                   FROM lineage WHERE lineage.of = found.id) AS path
           FROM found
           ORDER BY (found.name ILIKE $2 || '%' ESCAPE '\') DESC, lower(found.name), found.id",
    )
    .bind(owner_id)
    .bind(&escaped)
    .bind(limit.clamp(1, MAX_LIMIT))
    .bind(offset.max(0))
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
