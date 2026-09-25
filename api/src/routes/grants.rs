use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use uuid::Uuid;

use crate::access::{self, Place};
use crate::auth::{Caller, Writer};
use crate::error::ApiError;
use crate::grants;
use crate::state::AppState;
use roxycloud_core::grant::{Given, NewGrant, Received};
use roxycloud_core::name::parse_path;
use roxycloud_core::user::Email;

pub async fn create(
    State(state): State<AppState>,
    caller: Writer,
    Json(request): Json<NewGrant>,
) -> Result<(StatusCode, Json<Given>), ApiError> {
    let segments = parse_path(&request.path)?;
    if segments.is_empty() {
        return Err(ApiError::WrongKind {
            expected: "path below the root",
        });
    }
    let grantee: Email = request.email.parse()?;

    let mut tx = state.db.begin().await?;
    let Place::Own(node) =
        access::locate(&mut tx, &caller.user, &segments, state.default_quota_bytes).await?
    else {
        return Err(ApiError::Forbidden);
    };
    let given = grants::give(&mut tx, &caller.user, &node, &grantee, request.access).await?;
    tx.commit().await?;

    Ok((StatusCode::CREATED, Json(given)))
}

pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Vec<Given>>, ApiError> {
    Ok(Json(grants::given(&state.db, caller.user_id()).await?))
}

pub async fn received(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Vec<Received>>, ApiError> {
    Ok(Json(grants::received(&state.db, &caller.user.email).await?))
}

pub async fn withdraw(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    grants::withdraw(&state.db, &caller.user, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
