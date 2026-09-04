use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::ApiError;
use roxycloud_core::user::{Email, User};

const SECRET_BYTES: usize = 32;
const MAX_NAME_LEN: usize = 100;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AppPassword {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct Minted {
    #[serde(flatten)]
    pub password: AppPassword,
    /// The only time the secret exists outside the client that will use it.
    pub secret: String,
}

pub async fn mint(
    tx: &mut Transaction<'_, Postgres>,
    user_id: Uuid,
    name: &str,
) -> Result<Minted, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN {
        return Err(ApiError::WrongKind {
            expected: "name for the app password, up to 100 characters",
        });
    }

    let secret = secret();
    let password = sqlx::query_as::<_, AppPassword>(
        "INSERT INTO app_passwords (id, user_id, name, hash)
         VALUES ($1, $2, $3, $4)
         RETURNING id, name, created_at, last_used_at",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(name)
    .bind(fingerprint(&secret))
    .fetch_one(&mut **tx)
    .await?;

    Ok(Minted { password, secret })
}

pub async fn list(pool: &PgPool, user_id: Uuid) -> Result<Vec<AppPassword>, ApiError> {
    sqlx::query_as::<_, AppPassword>(
        "SELECT id, name, created_at, last_used_at
         FROM app_passwords
         WHERE user_id = $1 AND revoked_at IS NULL
         ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn revoke(pool: &PgPool, user_id: Uuid, id: Uuid) -> Result<(), ApiError> {
    let revoked = sqlx::query(
        "UPDATE app_passwords SET revoked_at = now()
         WHERE id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await?;

    if revoked.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

/// Answers the account a `WebDAV` client is entitled to act as, or nothing at all. The presented
/// secret is high entropy and server-generated, so it is fingerprinted rather than run through a
/// password hash: clients send it on every request, and no dictionary reaches it.
pub async fn authenticate(pool: &PgPool, email: &Email, presented: &str) -> Option<User> {
    let user = crate::users::by_email(pool, email).await.ok().flatten()?;
    if user.disabled_at.is_some() {
        return None;
    }

    let fingerprint = fingerprint(presented);
    let id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM app_passwords
         WHERE user_id = $1 AND hash = $2 AND revoked_at IS NULL",
    )
    .bind(user.id)
    .bind(&fingerprint)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()?;

    let _ = sqlx::query(
        "UPDATE app_passwords SET last_used_at = now()
         WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < now() - INTERVAL '5 minutes')",
    )
    .bind(id)
    .execute(pool)
    .await;

    Some(user)
}

fn secret() -> String {
    let mut bytes = [0u8; SECRET_BYTES];
    getrandom::fill(&mut bytes).expect("the operating system has randomness");
    hex(&bytes)
}

fn fingerprint(secret: &str) -> String {
    blake3::hash(secret.as_bytes()).to_hex().to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        use std::fmt::Write;
        let _ = write!(out, "{byte:02x}");
        out
    })
}
