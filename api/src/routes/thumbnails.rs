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
        .map_err(|_| ApiError::Credential)?
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

/// The same headers every other route that answers with derived user content carries. A thumbnail
/// is re-encoded rather than passed through, but the invariant is that no route on this origin
/// serves something a browser will render, and one exception is how that stops being true.
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
        crate::routes::files::never_rendered(),
        Body::from_stream(ReaderStream::new(file)),
    )
        .into_response())
}
