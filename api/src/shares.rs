use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::attempts::{self, Scope};
use crate::db::node_columns;
use crate::error::ApiError;
use crate::password;
use roxycloud_core::node::Node;

const TOKEN_BYTES: usize = 32;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Share {
    pub id: Uuid,
    pub node_id: Uuid,
    pub name: String,
    pub has_password: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct Minted {
    #[serde(flatten)]
    pub share: Share,
    /// The only time the token exists outside the link that will carry it.
    pub token: String,
}

pub async fn mint(
    tx: &mut Transaction<'_, Postgres>,
    created_by: Uuid,
    node: &Node,
    expires_at: Option<DateTime<Utc>>,
    password: Option<&str>,
) -> Result<Minted, ApiError> {
    if expires_at.is_some_and(|at| at <= Utc::now()) {
        return Err(ApiError::WrongKind {
            expected: "expiry in the future",
        });
    }

    let password_hash = match password {
        Some(secret) => {
            password::check_strength(secret, password::MIN_SHARE_PASSWORD_LEN)?;
            Some(password::hash(secret)?)
        }
        None => None,
    };

    let token = token();
    let (id, created_at) = sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
        "INSERT INTO shares (id, node_id, created_by, token_hash, password_hash, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6)
         RETURNING id, created_at",
    )
    .bind(Uuid::now_v7())
    .bind(node.id)
    .bind(created_by)
    .bind(fingerprint(&token))
    .bind(password_hash.as_deref())
    .bind(expires_at)
    .fetch_one(&mut **tx)
    .await?;

    Ok(Minted {
        share: Share {
            id,
            node_id: node.id,
            name: node.name.clone(),
            has_password: password_hash.is_some(),
            expires_at,
            created_at,
            last_used_at: None,
        },
        token,
    })
}

pub async fn list(pool: &PgPool, created_by: Uuid) -> Result<Vec<Share>, ApiError> {
    sqlx::query_as::<_, Share>(
        "SELECT shares.id, shares.node_id, nodes.name,
                shares.password_hash IS NOT NULL AS has_password,
                shares.expires_at, shares.created_at, shares.last_used_at
         FROM shares
         JOIN nodes ON nodes.id = shares.node_id
         WHERE shares.created_by = $1 AND shares.revoked_at IS NULL
         ORDER BY shares.created_at DESC",
    )
    .bind(created_by)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn revoke(pool: &PgPool, created_by: Uuid, id: Uuid) -> Result<(), ApiError> {
    let revoked = sqlx::query(
        "UPDATE shares SET revoked_at = now()
         WHERE id = $1 AND created_by = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(created_by)
    .execute(pool)
    .await?;

    if revoked.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

/// The node a link stands for, or nothing a caller can tell apart from a token that was never
/// minted. A revoked link, an expired one, one whose node went to the trash and one minted by an
/// account since disabled all answer the same way, because a link that still says "wrong password"
/// tells whoever guessed the token that they guessed it.
pub async fn open(pool: &PgPool, token: &str, presented: Option<&str>) -> Result<Node, ApiError> {
    let found = sqlx::query_as::<_, (Uuid, Uuid, Option<String>)>(
        "SELECT shares.id, shares.node_id, shares.password_hash
         FROM shares
         JOIN users ON users.id = shares.created_by
         JOIN nodes ON nodes.id = shares.node_id AND nodes.deleted_at IS NULL
         WHERE shares.token_hash = $1
           AND shares.revoked_at IS NULL
           AND (shares.expires_at IS NULL OR shares.expires_at > now())
           AND users.disabled_at IS NULL",
    )
    .bind(fingerprint(token))
    .fetch_optional(pool)
    .await?;

    let Some((id, node_id, password_hash)) = found else {
        if let Some(secret) = presented {
            password::verify_decoy(secret);
        }
        return Err(ApiError::NotFound);
    };

    if let Some(expected) = password_hash {
        // A visitor who sent no password has not guessed anything. This 401 is the only thing that
        // tells their client to ask for one, so counting it would shut a link out after ten first
        // visits, nobody having guessed at all.
        let Some(secret) = presented else {
            return Err(ApiError::SharePassword);
        };

        // Keyed on the fingerprint rather than the token, so the table that limits guessing is not
        // itself a list of working links.
        let subject = fingerprint(token);
        attempts::spend(pool, Scope::Share, &subject).await?;

        if !password::verify(secret, &expected) {
            return Err(ApiError::SharePassword);
        }
        attempts::succeeded(pool, Scope::Share, &subject).await?;
    }

    let node = sqlx::query_as::<_, Node>(concat!(
        "SELECT ",
        node_columns!(),
        " FROM nodes WHERE id = $1 AND deleted_at IS NULL"
    ))
    .bind(node_id)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)?;

    touch(pool, id).await;
    Ok(node)
}

async fn touch(pool: &PgPool, id: Uuid) {
    let _ = sqlx::query(
        "UPDATE shares SET last_used_at = now()
         WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < now() - INTERVAL '5 minutes')",
    )
    .bind(id)
    .execute(pool)
    .await;
}

fn token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).expect("the operating system has randomness");
    bytes.iter().fold(String::new(), |mut out, byte| {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// The token is high entropy and server-generated, so it is fingerprinted rather than run through
/// a password hash: it arrives on every request to the link, and no dictionary reaches it. The
/// optional password a person chooses is the opposite, and goes through `password::hash`.
fn fingerprint(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_unguessable_and_never_repeats() {
        let first = token();
        let second = token();

        assert_eq!(first.len(), TOKEN_BYTES * 2);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn the_stored_fingerprint_is_not_the_token() {
        let token = token();
        let stored = fingerprint(&token);

        assert_ne!(
            stored, token,
            "a stolen database row would be a working link"
        );
        assert_eq!(stored, fingerprint(&token));
        assert_ne!(stored, fingerprint("something else"));
    }
}
