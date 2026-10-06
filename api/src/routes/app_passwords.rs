use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use crate::app_passwords::{self, AppPassword, Minted};
use crate::auth::{Caller, Credential, SessionCaller};
use crate::error::ApiError;
use crate::state::AppState;
use crate::users;

#[derive(Deserialize)]
pub struct Name {
    name: String,
}

pub async fn mint(
    State(state): State<AppState>,
    caller: SessionCaller,
    Json(request): Json<Name>,
) -> Result<(StatusCode, Json<Minted>), ApiError> {
    // A credential outlives the session that minted it, so this is the one route where a token
    // that survived its account being disabled would hand out something durable.
    let account = users::by_id(&state.db, caller.user_id())
        .await?
        .ok_or(ApiError::Unauthenticated)?;
    if !account.is_active() {
        return Err(ApiError::Unauthenticated);
    }

    let mut tx = state.db.begin().await?;
    let minted = app_passwords::mint(&mut tx, caller.user_id(), &request.name).await?;
    tx.commit().await?;

    Ok((StatusCode::CREATED, Json(minted)))
}

pub async fn list(
    State(state): State<AppState>,
    caller: SessionCaller,
) -> Result<Json<Vec<AppPassword>>, ApiError> {
    Ok(Json(
        app_passwords::list(&state.db, caller.user_id()).await?,
    ))
}

pub async fn revoke(
    State(state): State<AppState>,
    caller: SessionCaller,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    app_passwords::revoke(&state.db, caller.user_id(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn revoke_presented(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<StatusCode, ApiError> {
    let Credential::AppPassword(id) = caller.via else {
        return Err(ApiError::AppPasswordRequired);
    };
    app_passwords::revoke(&state.db, caller.user.id, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
