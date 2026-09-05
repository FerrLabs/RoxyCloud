use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use uuid::Uuid;

use crate::auth::{Caller, Writer};
use crate::error::ApiError;
use crate::state::AppState;
use crate::trash;
use roxycloud_core::node::Node;

pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Vec<Node>>, ApiError> {
    Ok(Json(trash::list(&state.db, caller.user_id()).await?))
}

pub async fn restore(
    State(state): State<AppState>,
    caller: Writer,
    Path(id): Path<Uuid>,
) -> Result<Json<Node>, ApiError> {
    let mut tx = state.db.begin().await?;
    let restored = trash::restore(&mut tx, caller.user_id(), id).await?;
    tx.commit().await?;

    Ok(Json(restored))
}

pub async fn purge(
    State(state): State<AppState>,
    caller: Writer,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let mut tx = state.db.begin().await?;
    trash::purge(&mut tx, caller.user_id(), id).await?;
    tx.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}
