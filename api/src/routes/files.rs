use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio_util::io::ReaderStream;

use crate::auth::Caller;
use crate::db;
use crate::error::ApiError;
use crate::state::AppState;
use roxycloud_core::name::parse_path;
use roxycloud_core::node::{Node, NodeKind};

pub async fn put(
    State(state): State<AppState>,
    caller: Caller,
    Path(path): Path<String>,
    body: Body,
) -> Result<Response, ApiError> {
    let mut segments = parse_path(&path)?;
    let name = segments.pop().ok_or(ApiError::WrongKind {
        expected: "file path",
    })?;

    let written = state.blobs.write(body.into_data_stream()).await?;
    let size = i64::try_from(written.size).map_err(|_| ApiError::QuotaExceeded)?;

    let mut tx = state.db.begin().await?;
    let root = db::ensure_root(&mut tx, caller.user_id, state.default_quota_bytes).await?;
    let parent = db::create_directories(&mut tx, caller.user_id, &root, &segments).await?;
    let node = db::put_file(&mut tx, caller.user_id, &parent, &name, written.hash, size).await?;
    tx.commit().await?;

    let etag = HeaderValue::from_str(&node.etag).map_err(|_| ApiError::WrongKind {
        expected: "printable etag",
    })?;
    Ok((StatusCode::CREATED, [(header::ETAG, etag)], Json(node)).into_response())
}

pub async fn get(
    State(state): State<AppState>,
    caller: Caller,
    Path(path): Path<String>,
) -> Result<Response, ApiError> {
    let node = resolve_owned(&state, caller, &path).await?;
    let (NodeKind::File, Some(hash)) = (node.kind, node.blob_hash) else {
        return Err(ApiError::WrongKind { expected: "file" });
    };

    let file = state.blobs.read(hash).await?;
    let etag = HeaderValue::from_str(&node.etag).map_err(|_| ApiError::WrongKind {
        expected: "printable etag",
    })?;

    Ok((
        [
            (header::ETAG, etag),
            (
                header::CONTENT_LENGTH,
                HeaderValue::from(u64::try_from(node.size).unwrap_or(0)),
            ),
        ],
        Body::from_stream(ReaderStream::new(file)),
    )
        .into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    caller: Caller,
    Path(path): Path<String>,
) -> Result<StatusCode, ApiError> {
    let segments = parse_path(&path)?;
    if segments.is_empty() {
        return Err(ApiError::WrongKind {
            expected: "path below the root",
        });
    }

    let mut tx = state.db.begin().await?;
    let root = db::ensure_root(&mut tx, caller.user_id, state.default_quota_bytes).await?;
    let node = db::resolve(&mut tx, &root, &segments).await?;
    db::trash(&mut tx, &node).await?;
    tx.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Path(path): Path<String>,
) -> Result<Json<Vec<Node>>, ApiError> {
    let node = resolve_owned(&state, caller, &path).await?;
    if node.kind != NodeKind::Directory {
        return Err(ApiError::WrongKind {
            expected: "directory",
        });
    }
    Ok(Json(db::list_children(&state.db, node.id).await?))
}

pub async fn list_root(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Vec<Node>>, ApiError> {
    let mut tx = state.db.begin().await?;
    let root = db::ensure_root(&mut tx, caller.user_id, state.default_quota_bytes).await?;
    tx.commit().await?;
    Ok(Json(db::list_children(&state.db, root.id).await?))
}

async fn resolve_owned(state: &AppState, caller: Caller, path: &str) -> Result<Node, ApiError> {
    let segments = parse_path(path)?;
    let mut tx = state.db.begin().await?;
    let root = db::ensure_root(&mut tx, caller.user_id, state.default_quota_bytes).await?;
    let node = db::resolve(&mut tx, &root, &segments).await?;
    tx.commit().await?;
    Ok(node)
}
