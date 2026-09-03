use roxycloud_core::role::Role;
use roxycloud_core::user::{Email, User};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::error::ApiError;
use crate::password;

const USER_COLUMNS: &str = "id, email, display_name, password_hash, role,
     (role = 'admin') AS is_admin, created_at, disabled_at";

pub async fn by_email(pool: &PgPool, email: &Email) -> Result<Option<User>, ApiError> {
    sqlx::query_as::<_, User>(&format!(
        "SELECT {USER_COLUMNS} FROM users WHERE email = $1"
    ))
    .bind(email.as_str())
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

pub async fn by_id(pool: &PgPool, id: Uuid) -> Result<Option<User>, ApiError> {
    sqlx::query_as::<_, User>(&format!("SELECT {USER_COLUMNS} FROM users WHERE id = $1"))
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

    sqlx::query_as::<_, User>(&format!(
        "INSERT INTO users (id, email, display_name, password_hash, role)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING {USER_COLUMNS}"
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
