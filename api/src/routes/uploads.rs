use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::auth::{Caller, Writer};
use crate::error::ApiError;
use crate::state::AppState;
use crate::{db, storage, uploads};
use roxycloud_core::name::parse_path;

/// The offset a client is at, and the one it is told to come back to. Named after the header the
/// tus protocol uses, since a client that already speaks that shape should find this familiar.
const OFFSET: &str = "upload-offset";

#[derive(Deserialize)]
pub struct NewUpload {
    path: String,
    size: i64,
}

pub async fn begin(
    State(state): State<AppState>,
    caller: Writer,
    Json(request): Json<NewUpload>,
) -> Result<(StatusCode, Json<uploads::Session>), ApiError> {
    let segments = parse_path(&request.path)?;
    if segments.is_empty() {
        return Err(ApiError::WrongKind {
            expected: "path below the root",
        });
    }

    // Refused here rather than at the end, so a client does not send a file for an hour to be told
    // there was never room for it.
    let mut tx = state.db.begin().await?;
    db::ensure_root(&mut tx, caller.user_id(), state.default_quota_bytes).await?;
    db::room_for(&mut tx, caller.user_id(), request.size).await?;
    tx.commit().await?;

    let session = uploads::begin(
        &state.db,
        &state.staging,
        caller.user_id(),
        &request.path,
        request.size,
    )
    .await?;

    Ok((StatusCode::CREATED, Json(session)))
}

pub async fn status(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let session = uploads::of(&state.db, caller.user_id(), id).await?;
    Ok((offset_header(session.received), Json(session)).into_response())
}

pub async fn append(
    State(state): State<AppState>,
    caller: Writer,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, ApiError> {
    let session = uploads::of(&state.db, caller.user_id(), id).await?;
    let offset = headers
        .get(OFFSET)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or(ApiError::WrongKind {
            expected: "Upload-Offset header",
        })?;

    let advanced = uploads::append(
        &state.db,
        &state.staging,
        &session,
        offset,
        body.into_data_stream(),
    )
    .await?;

    Ok((
        StatusCode::NO_CONTENT,
        offset_header(advanced.received),
        Json(advanced),
    )
        .into_response())
}

/// Rehashes the staged file rather than carrying a hasher across requests. The digest has to be of
/// what actually landed, and a hasher state persisted between two processes is a second thing that
/// can disagree with the bytes.
pub async fn finish(
    State(state): State<AppState>,
    caller: Writer,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let session = uploads::of(&state.db, caller.user_id(), id).await?;
    if session.received != session.size {
        return Err(ApiError::Incomplete {
            received: session.received,
            expected: session.size,
        });
    }

    let mut segments = parse_path(&session.path)?;
    let name = segments.pop().ok_or(ApiError::WrongKind {
        expected: "path below the root",
    })?;

    let staged = uploads::staged_path(&state.staging, &session);
    let file = tokio::fs::File::open(&staged).await?;
    let written = state
        .blobs
        .write(storage::upload(ReaderStream::new(file)))
        .await?;
    let size = i64::try_from(written.size).map_err(|_| ApiError::QuotaExceeded)?;
    db::register_blob(&state.db, written.hash, size).await?;

    let mut tx = state.db.begin().await?;
    let root = db::ensure_root(&mut tx, caller.user_id(), state.default_quota_bytes).await?;
    let parent = db::create_directories(&mut tx, caller.user_id(), &root, &segments).await?;
    let node = db::put_file(
        &mut tx,
        caller.user_id(),
        &parent,
        &name,
        written.hash,
        size,
    )
    .await?;
    tx.commit().await?;
    state.blobs.settle(&written).await?;

    uploads::abandon(&state.db, &state.staging, &session).await?;

    Ok((StatusCode::CREATED, Json(node)).into_response())
}

pub async fn abandon(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let session = uploads::of(&state.db, caller.user_id(), id).await?;
    uploads::abandon(&state.db, &state.staging, &session).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn offset_header(offset: i64) -> [(header::HeaderName, HeaderValue); 1] {
    [(
        header::HeaderName::from_static(OFFSET),
        HeaderValue::from(offset),
    )]
}
