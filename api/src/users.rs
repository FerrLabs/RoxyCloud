use roxycloud_core::role::Role;
use roxycloud_core::user::{Email, User};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::ApiError;
use crate::password;

macro_rules! user_columns {
    () => {
        "id, email, display_name, password_hash, role, (role = 'admin') AS is_admin, created_at, disabled_at"
    };
}

pub async fn by_email(pool: &PgPool, email: &Email) -> Result<Option<User>, ApiError> {
    sqlx::query_as::<_, User>(concat!(
        "SELECT ",
        user_columns!(),
        " FROM users WHERE email = $1"
    ))
    .bind(email.as_str())
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn by_id(pool: &PgPool, id: Uuid) -> Result<Option<User>, ApiError> {
    sqlx::query_as::<_, User>(concat!(
        "SELECT ",
        user_columns!(),
        " FROM users WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn create(
    tx: &mut Transaction<'_, Postgres>,
    email: &Email,
    display_name: &str,
    plaintext: &str,
    role: Role,
) -> Result<User, ApiError> {
    password::check_strength(plaintext)?;
    let hash = password::hash(plaintext)?;

    sqlx::query_as::<_, User>(concat!(
        "INSERT INTO users (id, email, display_name, password_hash, role)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING ",
        user_columns!()
    ))
    .bind(Uuid::now_v7())
    .bind(email.as_str())
    .bind(display_name)
    .bind(hash)
    .bind(role)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| match err {
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            ApiError::Conflict(email.to_string())
        }
        other => other.into(),
    })
}

pub async fn count(pool: &PgPool) -> Result<i64, ApiError> {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM users")
        .fetch_one(pool)
        .await
        .map_err(Into::into)
}

pub async fn list(pool: &PgPool) -> Result<Vec<User>, ApiError> {
    sqlx::query_as::<_, User>(concat!(
        "SELECT ",
        user_columns!(),
        " FROM users ORDER BY created_at"
    ))
    .fetch_all(pool)
    .await
    .map_err(Into::into)
}

pub async fn set_disabled(pool: &PgPool, id: Uuid, disabled: bool) -> Result<User, ApiError> {
    sqlx::query_as::<_, User>(concat!(
        "UPDATE users SET disabled_at = CASE WHEN $2 THEN now() ELSE NULL END
         WHERE id = $1
         RETURNING ",
        user_columns!()
    ))
    .bind(id)
    .bind(disabled)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)
}

pub async fn set_role(pool: &PgPool, id: Uuid, role: Role) -> Result<User, ApiError> {
    sqlx::query_as::<_, User>(concat!(
        "UPDATE users SET role = $2 WHERE id = $1 RETURNING ",
        user_columns!()
    ))
    .bind(id)
    .bind(role)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::NotFound)
}

pub async fn set_password(pool: &PgPool, id: Uuid, plaintext: &str) -> Result<(), ApiError> {
    password::check_strength(plaintext)?;
    let hash = password::hash(plaintext)?;

    let changed = sqlx::query("UPDATE users SET password_hash = $2 WHERE id = $1")
        .bind(id)
        .bind(hash)
        .execute(pool)
        .await?;

    if changed.rows_affected() == 0 {
        return Err(ApiError::NotFound);
    }
    Ok(())
}

/// The quota row is created on an account's first write, so setting one before then has to make it.
pub async fn set_quota(pool: &PgPool, id: Uuid, bytes_max: i64) -> Result<(), ApiError> {
    if bytes_max <= 0 {
        return Err(ApiError::WrongKind {
            expected: "quota above zero",
        });
    }

    sqlx::query(
        "INSERT INTO quotas (owner_id, bytes_max) VALUES ($1, $2)
         ON CONFLICT (owner_id) DO UPDATE SET bytes_max = $2, updated_at = now()",
    )
    .bind(id)
    .bind(bytes_max)
    .execute(pool)
    .await
    .map_err(|error| match error {
        sqlx::Error::Database(ref db) if db.is_foreign_key_violation() => ApiError::NotFound,
        other => other.into(),
    })?;
    Ok(())
}

pub async fn usage(pool: &PgPool, id: Uuid) -> Result<Option<(i64, i64)>, ApiError> {
    sqlx::query_as::<_, (i64, i64)>("SELECT bytes_used, bytes_max FROM quotas WHERE owner_id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(Into::into)
}
