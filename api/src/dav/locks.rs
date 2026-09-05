use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::ApiError;
use roxycloud_core::node::Node;

/// What a client gets if it asks for nothing in particular, and the most any client may hold. A
/// lock nobody refreshes is a file nobody else can write, so the ceiling is what limits the damage
/// a client that disappears can do.
const DEFAULT_SECONDS: i64 = 600;
const MAX_SECONDS: i64 = 3600;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Lock {
    pub token: String,
    pub node_id: Uuid,
    pub owner_id: Uuid,
    pub holder: Option<String>,
    pub deep: bool,
    pub expires_at: DateTime<Utc>,
}

impl Lock {
    #[must_use]
    pub fn seconds_left(&self) -> i64 {
        (self.expires_at - Utc::now()).num_seconds().max(0)
    }
}

/// `Timeout: Second-600`, or `Infinite`, which is answered with the ceiling rather than refused:
/// a client that asked for forever copes with being given an hour, where a 400 stops it dead.
#[must_use]
pub fn requested_seconds(header: Option<&str>) -> i64 {
    header
        .and_then(|raw| {
            raw.split(',').find_map(|entry| {
                let entry = entry.trim();
                entry
                    .strip_prefix("Second-")
                    .or_else(|| entry.strip_prefix("second-"))
                    .and_then(|seconds| seconds.parse::<i64>().ok())
            })
        })
        .unwrap_or(DEFAULT_SECONDS)
        .clamp(1, MAX_SECONDS)
}

pub async fn take(
    tx: &mut Transaction<'_, Postgres>,
    node: &Node,
    owner_id: Uuid,
    holder: Option<&str>,
    deep: bool,
    seconds: i64,
) -> Result<Lock, ApiError> {
    let seconds = f64::from(i32::try_from(seconds).unwrap_or(i32::MAX));
    let token = format!("opaquelocktoken:{}", Uuid::now_v7());

    sqlx::query_as::<_, Lock>(
        "INSERT INTO locks (token, node_id, owner_id, holder, deep, expires_at)
         VALUES ($1, $2, $3, $4, $5, now() + make_interval(secs => $6))
         RETURNING token, node_id, owner_id, holder, deep, expires_at",
    )
    .bind(&token)
    .bind(node.id)
    .bind(owner_id)
    .bind(holder)
    .bind(deep)
    .bind(seconds)
    .fetch_one(&mut **tx)
    .await
    .map_err(
        |error| match error.as_database_error().and_then(|db| db.constraint()) {
            Some("locks_one_per_node") => ApiError::Locked(node.name.clone()),
            _ => ApiError::Database(error),
        },
    )
}

pub async fn refresh(
    tx: &mut Transaction<'_, Postgres>,
    token: &str,
    node: &Node,
    seconds: i64,
) -> Result<Option<Lock>, ApiError> {
    let seconds = f64::from(i32::try_from(seconds).unwrap_or(i32::MAX));
    sqlx::query_as::<_, Lock>(
        "UPDATE locks SET expires_at = now() + make_interval(secs => $3)
         WHERE token = $1 AND node_id = $2 AND expires_at > now()
         RETURNING token, node_id, owner_id, holder, deep, expires_at",
    )
    .bind(token)
    .bind(node.id)
    .bind(seconds)
    .fetch_optional(&mut **tx)
    .await
    .map_err(Into::into)
}

pub async fn release(pool: &PgPool, node: &Node, token: &str) -> Result<bool, ApiError> {
    let released = sqlx::query("DELETE FROM locks WHERE token = $1 AND node_id = $2")
        .bind(token)
        .bind(node.id)
        .execute(pool)
        .await?;

    Ok(released.rows_affected() > 0)
}

pub async fn on(tx: &mut Transaction<'_, Postgres>, node: &Node) -> Result<Option<Lock>, ApiError> {
    sqlx::query_as::<_, Lock>(
        "SELECT token, node_id, owner_id, holder, deep, expires_at
         FROM locks WHERE node_id = $1 AND expires_at > now()",
    )
    .bind(node.id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(Into::into)
}

/// The lock standing in the way of writing to this node: its own, or one taken with `Depth:
/// infinity` on something above it. An expired row is not in the way, which is what keeps a client
/// that disappeared from holding a file forever even before the sweep removes the row.
pub async fn covering(
    tx: &mut Transaction<'_, Postgres>,
    node: &Node,
) -> Result<Option<Lock>, ApiError> {
    sqlx::query_as::<_, Lock>(
        "WITH RECURSIVE ancestry AS (
             SELECT id, parent_id FROM nodes WHERE id = $1
             UNION ALL
             SELECT ancestor.id, ancestor.parent_id
             FROM nodes ancestor
             JOIN ancestry ON ancestor.id = ancestry.parent_id
         ) CYCLE id SET looped USING trail
         SELECT locks.token, locks.node_id, locks.owner_id, locks.holder, locks.deep,
                locks.expires_at
         FROM locks
         JOIN ancestry ON ancestry.id = locks.node_id
         WHERE locks.expires_at > now() AND (locks.node_id = $1 OR locks.deep)
         LIMIT 1",
    )
    .bind(node.id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(Into::into)
}

/// A write goes through when nothing holds the node, or when the client submitted the token that
/// does. Anything else is 423, whoever is asking: a lock the same account took from another client
/// is still a lock.
pub async fn allows(
    tx: &mut Transaction<'_, Postgres>,
    node: &Node,
    submitted: &[String],
) -> Result<(), ApiError> {
    let Some(lock) = covering(tx, node).await? else {
        return Ok(());
    };

    if submitted.contains(&lock.token) {
        return Ok(());
    }
    Err(ApiError::Locked(node.name.clone()))
}

/// Deleting a collection takes everything under it, so a lock anywhere in that subtree stands in
/// the way. Without this a client could drop a folder to get at a file another client holds.
pub async fn none_below(
    tx: &mut Transaction<'_, Postgres>,
    node: &Node,
    submitted: &[String],
) -> Result<(), ApiError> {
    let held = sqlx::query_as::<_, (String, String)>(
        "WITH RECURSIVE subtree AS (
             SELECT id FROM nodes WHERE id = $1
             UNION ALL
             SELECT descendant.id
             FROM nodes descendant
             JOIN subtree ON descendant.parent_id = subtree.id
             WHERE descendant.deleted_at IS NULL
         ) CYCLE id SET looped USING trail
         SELECT locks.token, nodes.name
         FROM locks
         JOIN subtree ON subtree.id = locks.node_id
         JOIN nodes ON nodes.id = locks.node_id
         WHERE locks.expires_at > now()",
    )
    .bind(node.id)
    .fetch_all(&mut **tx)
    .await?;

    for (token, name) in held {
        if !submitted.contains(&token) {
            return Err(ApiError::Locked(name));
        }
    }
    Ok(())
}

/// One query for a whole listing: a lock lookup per child would make a directory walk cost a
/// round trip per entry.
pub async fn for_nodes(
    tx: &mut Transaction<'_, Postgres>,
    ids: &[Uuid],
) -> Result<Vec<Lock>, ApiError> {
    sqlx::query_as::<_, Lock>(
        "SELECT token, node_id, owner_id, holder, deep, expires_at
         FROM locks WHERE node_id = ANY($1) AND expires_at > now()",
    )
    .bind(ids)
    .fetch_all(&mut **tx)
    .await
    .map_err(Into::into)
}

pub async fn purge_expired(pool: &PgPool) -> Result<u64, ApiError> {
    let removed = sqlx::query("DELETE FROM locks WHERE expires_at <= now()")
        .execute(pool)
        .await?;
    Ok(removed.rows_affected())
}

/// Every token a client submitted, from `If: (<opaquelocktoken:...>)` or the tagged form. `ETag`
/// conditions and `Not` are not read: a client that sends one gets the same answer as one that
/// sent nothing, which is a refusal rather than a write it did not ask for.
#[must_use]
pub fn submitted_tokens(header: Option<&str>) -> Vec<String> {
    let Some(raw) = header else {
        return Vec::new();
    };

    let mut tokens = Vec::new();
    let mut rest = raw;
    while let Some(start) = rest.find('<') {
        let Some(end) = rest[start + 1..].find('>') else {
            break;
        };
        let candidate = &rest[start + 1..start + 1 + end];
        if candidate.starts_with("opaquelocktoken:") {
            tokens.push(candidate.to_owned());
        }
        rest = &rest[start + 1 + end + 1..];
    }
    tokens
}

/// The `Lock-Token: <opaquelocktoken:...>` an UNLOCK carries.
#[must_use]
pub fn lock_token(header: Option<&str>) -> Option<String> {
    let raw = header?.trim();
    let inner = raw.strip_prefix('<')?.strip_suffix('>')?;
    inner
        .starts_with("opaquelocktoken:")
        .then(|| inner.to_owned())
}

#[must_use]
pub fn timeout_header(lock: &Lock) -> String {
    format!("Second-{}", lock.seconds_left())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timeout_of_nothing_is_the_default() {
        assert_eq!(requested_seconds(None), DEFAULT_SECONDS);
        assert_eq!(requested_seconds(Some("")), DEFAULT_SECONDS);
    }

    #[test]
    fn a_client_gets_the_seconds_it_asked_for() {
        assert_eq!(requested_seconds(Some("Second-30")), 30);
        assert_eq!(requested_seconds(Some("second-30")), 30);
    }

    #[test]
    fn forever_is_answered_with_the_ceiling_rather_than_refused() {
        assert_eq!(requested_seconds(Some("Infinite")), DEFAULT_SECONDS);
        assert_eq!(requested_seconds(Some("Second-99999")), MAX_SECONDS);
        assert_eq!(
            requested_seconds(Some("Infinite, Second-120")),
            120,
            "a client offering a fallback is taken at the fallback"
        );
    }

    #[test]
    fn a_submitted_token_is_found_in_either_form() {
        assert_eq!(
            submitted_tokens(Some("(<opaquelocktoken:abc>)")),
            vec!["opaquelocktoken:abc".to_owned()]
        );
        assert_eq!(
            submitted_tokens(Some("</dav/a.txt> (<opaquelocktoken:abc>)")),
            vec!["opaquelocktoken:abc".to_owned()]
        );
    }

    #[test]
    fn several_tokens_all_come_through() {
        assert_eq!(
            submitted_tokens(Some(
                "</dav/a> (<opaquelocktoken:one>) </dav/b> (<opaquelocktoken:two>)"
            )),
            vec![
                "opaquelocktoken:one".to_owned(),
                "opaquelocktoken:two".to_owned()
            ]
        );
    }

    #[test]
    fn an_etag_condition_is_not_mistaken_for_a_token() {
        assert!(submitted_tokens(Some(r#"(["etag-value"])"#)).is_empty());
        assert!(submitted_tokens(Some("(<urn:something-else>)")).is_empty());
    }

    #[test]
    fn nothing_submitted_is_no_tokens_rather_than_a_failure() {
        assert!(submitted_tokens(None).is_empty());
        assert!(submitted_tokens(Some("garbage <unclosed")).is_empty());
    }

    #[test]
    fn an_unlock_reads_the_token_between_the_brackets() {
        assert_eq!(
            lock_token(Some("<opaquelocktoken:abc>")),
            Some("opaquelocktoken:abc".to_owned())
        );
        assert_eq!(
            lock_token(Some(" <opaquelocktoken:abc> ")),
            Some("opaquelocktoken:abc".to_owned())
        );
        assert_eq!(lock_token(Some("opaquelocktoken:abc")), None);
        assert_eq!(lock_token(Some("<urn:other>")), None);
        assert_eq!(lock_token(None), None);
    }
}
