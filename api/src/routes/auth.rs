use axum::Json;
use axum::extract::State;
use roxycloud_core::user::{Email, User};
use serde::{Deserialize, Serialize};

use crate::auth::Caller;
use crate::error::ApiError;
use crate::state::AppState;
use crate::{password, users};

#[derive(Deserialize)]
pub struct Credentials {
    email: String,
    password: String,
}

#[derive(Serialize)]
pub struct Session {
    token: String,
    expires_in: i64,
    user: User,
}

pub async fn login(
    State(state): State<AppState>,
    Json(credentials): Json<Credentials>,
) -> Result<Json<Session>, ApiError> {
    let email: Email = credentials
        .email
        .parse()
        .map_err(|_| ApiError::InvalidCredentials)?;

    let user = users::by_email(&state.db, &email).await?;

    let Some(user) = user.filter(User::is_active) else {
        password::verify_decoy(&credentials.password);
        return Err(ApiError::InvalidCredentials);
    };

    if !password::verify(&credentials.password, &user.password_hash) {
        return Err(ApiError::InvalidCredentials);
    }

    Ok(Json(Session {
        token: state.sessions.issue(user.id)?,
        expires_in: state.sessions.ttl_seconds(),
        user,
    }))
}

pub async fn me(State(state): State<AppState>, caller: Caller) -> Result<Json<User>, ApiError> {
    users::by_id(&state.db, caller.user_id)
        .await?
        .filter(User::is_active)
        .map(Json)
        .ok_or(ApiError::Unauthenticated)
}
