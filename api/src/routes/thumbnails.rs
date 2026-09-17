use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio_util::io::ReaderStream;

use crate::auth::Caller;
use crate::error::ApiError;
use crate::state::AppState;
use crate::{db, storage, thumbnails};
use roxycloud_core::name::parse_path;
use roxycloud_core::node::NodeKind;

#[derive(Deserialize)]
pub struct Wanted {
    edge: Option<i32>,
}

/// Made when it is asked for rather than when the file arrives, and cached against the source
/// digest, so the same photo uploaded by two people costs one thumbnail.
pub async fn get(
    State(state): State<AppState>,
    caller: Caller,
    Path(path): Path<String>,
    Query(wanted): Query<Wanted>,
) -> Result<Response, ApiError> {
    let edge = wanted.edge.unwrap_or(256);
    if !thumbnails::is_an_edge(edge) {
        return Err(ApiError::WrongKind {
            expected: "edge of 128, 256 or 512",
        });
    }

    let segments = parse_path(&path)?;
    let mut tx = state.db.begin().await?;
    let root = db::ensure_root(&mut tx, caller.user_id(), state.default_quota_bytes).await?;
    let node = db::resolve(&mut tx, &root, &segments).await?;
    tx.commit().await?;

    let (NodeKind::File, Some(source)) = (node.kind, node.blob_hash) else {
        return Err(ApiError::WrongKind { expected: "file" });
    };
    if !thumbnails::looks_like_an_image(&node.name) {
        return Err(ApiError::NotAnImage(thumbnails::NotAnImage::Format));
    }

    if let Some(cached) = thumbnails::cached(&state.db, source, edge).await? {
        return serve(&state, cached).await;
    }

    // Taken before the source is read, so the bytes of a queued request are not sitting in memory
    // waiting their turn. The cache hit above never reaches this, which is the point: the cheap
    // path should not queue behind the expensive one.
    let _decoding = state
        .decoders
        .acquire()
        .await
        .map_err(|_| ApiError::Credential)?;

    let mut bytes = Vec::new();
    state
        .blobs
        .read(source)
        .await?
        .take(thumbnails::LARGEST_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .await?;

    // Off the async runtime: decoding is the one thing here that spends real CPU, and a request
    // that stalls the executor stalls every other request with it.
    let edge_for_shrink = u32::try_from(edge).unwrap_or(256);
    let shrunk = tokio::task::spawn_blocking(move || thumbnails::shrink(&bytes, edge_for_shrink))
        .await
        .map_err(|_| {
            ApiError::Storage(crate::storage::StorageError::Remote(
                "the decoder did not finish".to_owned(),
            ))
        })?
        .map_err(ApiError::NotAnImage)?;

    let size = i64::try_from(shrunk.len()).map_err(|_| ApiError::QuotaExceeded)?;
    let written = state
        .blobs
        .write(storage::upload(futures::stream::once(async move {
            Ok::<_, std::io::Error>(bytes::Bytes::from(shrunk))
        })))
        .await?;
    db::register_blob(&state.db, written.hash, size).await?;

    let mut tx = state.db.begin().await?;
    thumbnails::remember(&mut tx, source, edge, written.hash).await?;
    tx.commit().await?;
    state.blobs.settle(&written).await?;

    serve(&state, written.hash).await
}

/// The real type, unlike every other route that answers with bytes.
///
/// The reasoning that puts `application/octet-stream` on the rest is that the bytes came from a
/// person and we will not vouch for them. These came out of our own encoder, and `WebP` carries no
/// script surface, so that argument does not reach this response. `nosniff` and the attachment
/// disposition stay, and sending the real type is what lets a client point an `<img>` at this at
/// all: Chrome refuses to load one when `nosniff` is set and the type is not an image.
async fn serve(
    state: &AppState,
    hash: roxycloud_core::blob::BlobHash,
) -> Result<Response, ApiError> {
    let file = state.blobs.read(hash).await?;
    Ok((
        [(
            header::CACHE_CONTROL,
            HeaderValue::from_static("private, max-age=86400"),
        )],
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("image/webp")),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_static("attachment"),
            ),
            (
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            ),
        ],
        Body::from_stream(ReaderStream::new(file)),
    )
        .into_response())
}
