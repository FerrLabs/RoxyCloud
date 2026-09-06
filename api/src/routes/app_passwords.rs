use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use uuid::Uuid;

use crate::app_passwords::{self, AppPassword, Minted};
use crate::auth::Caller;
use crate::error::ApiError;
use crate::state::AppState;
use crate::users;

#[derive(Deserialize)]
pub struct Name {
    name: String,
}

pub async fn mint(
    State(state): State<AppState>,
    caller: Caller,
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
    caller: Caller,
) -> Result<Json<Vec<AppPassword>>, ApiError> {
    Ok(Json(
        app_passwords::list(&state.db, caller.user_id()).await?,
    ))
}

pub async fn revoke(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    app_passwords::revoke(&state.db, caller.user_id(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}
