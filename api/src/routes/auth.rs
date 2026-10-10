use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use stashden_core::user::{Email, User};

use crate::attempts::{self, Scope};
use crate::auth::{Caller, SessionCaller};
use crate::error::ApiError;
use crate::state::AppState;
use crate::{password, sessions, users};

#[derive(Deserialize)]
pub struct Credentials {
    email: String,
    password: String,
}

#[derive(Serialize)]
pub struct Session {
    pub token: String,
    pub expires_in: i64,
    pub user: User,
}

pub async fn login(
    State(state): State<AppState>,
    Json(credentials): Json<Credentials>,
) -> Result<Json<Session>, ApiError> {
    let email: Email = credentials
        .email
        .parse()
        .map_err(|_| ApiError::InvalidCredentials)?;

    // Counted before the lookup and before argon2, because a guess that costs the server a hash is
    // a guess worth making. Unknown addresses are counted like known ones, so a 429 never says
    // which is which.
    // An installation that has turned passwords off has a provider instead, and a route that
    // still took them would be the way around that decision.
    if !crate::settings::password_login_allowed(&state.db).await? {
        return Err(ApiError::NotFound);
    }

    attempts::spend(&state.db, Scope::Login, email.as_str()).await?;

    let user = users::by_email(&state.db, &email).await?;

    let Some(user) = user.filter(User::is_active) else {
        password::verify_decoy(&credentials.password);
        return Err(ApiError::InvalidCredentials);
    };

    if !password::verify(&credentials.password, &user.password_hash) {
        return Err(ApiError::InvalidCredentials);
    }
    attempts::forget(&state.db, Scope::Login, email.as_str()).await?;

    Ok(Json(Session {
        token: state.sessions.open(&state.db, user.id).await?,
        expires_in: state.sessions.ttl_seconds(),
        user,
    }))
}

pub async fn logout(
    State(state): State<AppState>,
    caller: SessionCaller,
) -> Result<StatusCode, ApiError> {
    sessions::revoke(&state.db, caller.session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct ProfileUpdate {
    display_name: String,
}

pub async fn update_profile(
    State(state): State<AppState>,
    caller: SessionCaller,
    Json(request): Json<ProfileUpdate>,
) -> Result<Json<User>, ApiError> {
    Ok(Json(
        users::set_display_name(&state.db, caller.user_id(), &request.display_name).await?,
    ))
}

pub async fn me(State(state): State<AppState>, caller: Caller) -> Result<Json<User>, ApiError> {
    users::by_id(&state.db, caller.user_id())
        .await?
        .filter(User::is_active)
        .map(Json)
        .ok_or(ApiError::Unauthenticated)
}
