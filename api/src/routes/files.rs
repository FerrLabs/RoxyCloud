use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio_util::io::ReaderStream;

use crate::access::{self, Place};
use crate::auth::{Caller, Writer};
use crate::db;
use crate::error::ApiError;
use crate::state::AppState;
use crate::storage;
use crate::trash;
use roxycloud_core::blob::BlobHash;
use roxycloud_core::name::parse_path;
use roxycloud_core::node::{Node, NodeKind};

pub async fn put(
    State(state): State<AppState>,
    caller: Writer,
    Path(path): Path<String>,
    body: Body,
) -> Result<Response, ApiError> {
    let segments = parse_path(&path)?;
    if segments.is_empty() {
        return Err(ApiError::WrongKind {
            expected: "file path",
        });
    }

    let written = state
        .blobs
        .write(storage::upload(body.into_data_stream()))
        .await?;
    let size = i64::try_from(written.size).map_err(|_| ApiError::QuotaExceeded)?;
    db::register_blob(&state.db, written.hash, size).await?;

    let mut tx = state.db.begin().await?;
    let (parent, name) = access::file_target(
        &mut tx,
        &caller.user,
        &segments,
        true,
        state.default_quota_bytes,
    )
    .await?;
    let node = db::put_file(
        &mut tx,
        parent.owner_id,
        &parent,
        &name,
        written.hash,
        size,
        state.versions_kept,
    )
    .await?;
    tx.commit().await?;
    state.blobs.settle(&written).await?;

    let etag = etag_header(&node)?;
    Ok((StatusCode::CREATED, [(header::ETAG, etag)], Json(node)).into_response())
}

pub async fn get(
    State(state): State<AppState>,
    caller: Caller,
    Path(path): Path<String>,
) -> Result<Response, ApiError> {
    let node = located(&state, &caller, &path).await?.into_node()?;
    bytes_of(&state, &node).await
}

/// Every route that answers with the contents of a file goes through here, share links included,
/// so the headers that keep uploaded bytes from rendering are attached once rather than remembered
/// by each caller.
pub(crate) async fn bytes_of(state: &AppState, node: &Node) -> Result<Response, ApiError> {
    let (NodeKind::File, Some(hash)) = (node.kind, node.blob_hash) else {
        return Err(ApiError::WrongKind { expected: "file" });
    };
    blob_bytes(state, hash, node.size, etag_header(node)?).await
}

pub(crate) async fn blob_bytes(
    state: &AppState,
    hash: BlobHash,
    size: i64,
    etag: HeaderValue,
) -> Result<Response, ApiError> {
    let file = state.blobs.read(hash).await?;

    Ok((
        [
            (header::ETAG, etag),
            (
                header::CONTENT_LENGTH,
                HeaderValue::from(u64::try_from(size).unwrap_or(0)),
            ),
        ],
        never_rendered(),
        Body::from_stream(ReaderStream::new(file)),
    )
        .into_response())
}

/// Every route that answers with bytes a user uploaded carries these, because the API serves the
/// web app and a page rendered here would run with its session in reach. See `ARCHITECTURE.md`.
#[must_use]
pub fn never_rendered() -> [(HeaderName, HeaderValue); 3] {
    [
        (
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        ),
        (
            header::CONTENT_DISPOSITION,
            HeaderValue::from_static("attachment"),
        ),
        (
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ),
    ]
}

fn etag_header(node: &Node) -> Result<HeaderValue, ApiError> {
    HeaderValue::from_str(&node.etag).map_err(|_| ApiError::WrongKind {
        expected: "printable etag",
    })
}

#[derive(Deserialize)]
pub struct Move {
    from: String,
    to: String,
}

pub async fn rename(
    State(state): State<AppState>,
    caller: Writer,
    Json(request): Json<Move>,
) -> Result<Response, ApiError> {
    let source = parse_path(&request.from)?;
    let mut destination = parse_path(&request.to)?;
    let name = destination.pop().ok_or(ApiError::WrongKind {
        expected: "path below the root",
    })?;
    if source.is_empty() {
        return Err(ApiError::WrongKind {
            expected: "path below the root",
        });
    }

    let quota = state.default_quota_bytes;
    let mut tx = state.db.begin().await?;
    let node = access::locate_writable(&mut tx, &caller.user, &source, quota)
        .await?
        .into_movable()?;
    let parent =
        access::directory_for_write(&mut tx, &caller.user, &destination, false, quota).await?;
    access::refuse_reserved(&parent, &name)?;
    if parent.owner_id != node.owner_id {
        return Err(ApiError::AcrossAccounts);
    }
    let moved = db::rename(&mut tx, &node, &parent, &name).await?;
    tx.commit().await?;

    let etag = etag_header(&moved)?;
    Ok(([(header::ETAG, etag)], Json(moved)).into_response())
}

pub async fn delete(
    State(state): State<AppState>,
    caller: Writer,
    Path(path): Path<String>,
) -> Result<StatusCode, ApiError> {
    let segments = parse_path(&path)?;
    if segments.is_empty() {
        return Err(ApiError::WrongKind {
            expected: "path below the root",
        });
    }

    let mut tx = state.db.begin().await?;
    let node = access::locate_writable(&mut tx, &caller.user, &segments, state.default_quota_bytes)
        .await?
        .into_movable()?;
    trash::send(&mut tx, &node).await?;
    tx.commit().await?;

    Ok(StatusCode::NO_CONTENT)
}

pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
    Path(path): Path<String>,
) -> Result<Json<Vec<Node>>, ApiError> {
    let segments = parse_path(&path)?;
    let quota = state.default_quota_bytes;
    let mut tx = state.db.begin().await?;
    let place = access::locate(&mut tx, &caller.user, &segments, quota).await?;

    if let Place::SharedWithMe = place {
        let root = db::ensure_root(&mut tx, caller.user_id(), quota).await?;
        let mounted = access::shelf(&mut tx, &caller.user, &root).await?;
        tx.commit().await?;
        return Ok(Json(
            mounted
                .map(|(_, entries)| entries.into_iter().map(|(node, _)| node).collect())
                .unwrap_or_default(),
        ));
    }
    tx.commit().await?;

    let node = place.into_node()?;
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
    let root = db::ensure_root(&mut tx, caller.user_id(), state.default_quota_bytes).await?;
    let shelf = access::shelf(&mut tx, &caller.user, &root).await?;
    tx.commit().await?;

    let mut listing: Vec<Node> = shelf.map(|(directory, _)| directory).into_iter().collect();
    listing.extend(db::list_children(&state.db, root.id).await?);
    Ok(Json(listing))
}

async fn located(state: &AppState, caller: &Caller, path: &str) -> Result<Place, ApiError> {
    let segments = parse_path(path)?;
    let mut tx = state.db.begin().await?;
    let place = access::locate(&mut tx, &caller.user, &segments, state.default_quota_bytes).await?;
    tx.commit().await?;
    Ok(place)
}
