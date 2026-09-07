use sqlx::PgPool;

use crate::error::ApiError;

const PASSWORD_LOGIN: &str = "password_login";

/// On unless somebody turned it off. A deployment with no provider configured has nothing else to
/// sign in with, so the absent row cannot mean "off".
pub async fn password_login_allowed(pool: &PgPool) -> Result<bool, ApiError> {
    let stored = sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = $1")
        .bind(PASSWORD_LOGIN)
        .fetch_optional(pool)
        .await?;

    Ok(stored.is_none_or(|value| value != "off"))
}

pub async fn allow_password_login(pool: &PgPool, allowed: bool) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ($1, $2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
    )
    .bind(PASSWORD_LOGIN)
    .bind(if allowed { "on" } else { "off" })
    .execute(pool)
    .await?;
    Ok(())
}
