use sqlx::PgPool;
use stashden_core::user::{Email, User};

use crate::attempts::{self, Scope};
use crate::error::ApiError;
use crate::{sessions, users};

pub async fn reset_password(
    pool: &PgPool,
    email: &Email,
    plaintext: &str,
) -> Result<User, ApiError> {
    let user = users::by_email(pool, email)
        .await?
        .ok_or(ApiError::NotFound)?;

    let mut tx = pool.begin().await?;
    users::set_password(&mut *tx, user.id, plaintext).await?;
    sessions::revoke_all(&mut *tx, user.id, None).await?;
    tx.commit().await?;

    attempts::forget(pool, Scope::Login, email.as_str()).await?;
    Ok(user)
}
