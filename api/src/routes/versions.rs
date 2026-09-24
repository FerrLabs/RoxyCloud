use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderValue;
use axum::response::Response;
use uuid::Uuid;

use crate::access;
use crate::auth::{Caller, Writer};
use crate::db;
use crate::error::ApiError;
use crate::routes::files::blob_bytes;
use crate::state::AppState;
use crate::versions::{self, Version};
use roxycloud_core::name::{NodeName, parse_path};
use roxycloud_core::node::{Node, NodeKind, etag_for_file};

pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Path(path): Path<String>,
) -> Result<Json<Vec<Version>>, ApiError> {
    let segments = parse_path(&path)?;
    let mut tx = state.db.begin().await?;
    let node = readable_file(&state, &caller, &segments, &mut tx).await?;
    let kept = versions::list(&mut tx, node.id).await?;
    tx.commit().await?;
    Ok(Json(kept))
}

pub async fn content(
    State(state): State<AppState>,
    caller: Caller,
    Path((id, path)): Path<(Uuid, String)>,
) -> Result<Response, ApiError> {
    let segments = parse_path(&path)?;
    let mut tx = state.db.begin().await?;
    let node = readable_file(&state, &caller, &segments, &mut tx).await?;
    let version = versions::find(&mut tx, node.id, id).await?;
    tx.commit().await?;

    let etag = HeaderValue::from_str(&etag_for_file(version.blob_hash)).map_err(|_| {
        ApiError::WrongKind {
            expected: "printable etag",
        }
    })?;
    blob_bytes(&state, version.blob_hash, version.size, etag).await
}

pub async fn restore(
    State(state): State<AppState>,
    caller: Writer,
    Path((id, path)): Path<(Uuid, String)>,
) -> Result<Json<Node>, ApiError> {
    let segments = parse_path(&path)?;
    let quota = state.default_quota_bytes;
    let mut tx = state.db.begin().await?;
    let node = file(
        access::locate_writable(&mut tx, &caller.user, &segments, quota)
            .await?
            .into_node()?,
    )?;
    let version = versions::take(&mut tx, node.owner_id, node.id, id).await?;
    let (parent, name) =
        access::file_target(&mut tx, &caller.user, &segments, false, quota).await?;
    let restored = db::put_file(
        &mut tx,
        parent.owner_id,
        &parent,
        &name,
        version.blob_hash,
        version.size,
        state.versions_kept,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(restored))
}

async fn readable_file(
    state: &AppState,
    caller: &Caller,
    segments: &[NodeName],
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> Result<Node, ApiError> {
    file(
        access::locate(tx, &caller.user, segments, state.default_quota_bytes)
            .await?
            .into_node()?,
    )
}

fn file(node: Node) -> Result<Node, ApiError> {
    if node.kind == NodeKind::File {
        Ok(node)
    } else {
        Err(ApiError::WrongKind { expected: "file" })
    }
}
