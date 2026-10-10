use axum::body::Body;
use axum::http::{HeaderMap, header};
use sqlx::{Postgres, Transaction};
use stashden_core::name::NodeName;
use stashden_core::node::Node;

use crate::db;
use crate::error::ApiError;
use crate::state::AppState;
use crate::storage::{self, StorageError, Written};
use crate::versions;

pub struct Received {
    pub written: Written,
    pub size: i64,
}

pub async fn room(
    tx: &mut Transaction<'_, Postgres>,
    state: &AppState,
    parent: &Node,
    name: &NodeName,
) -> Result<u64, ApiError> {
    let left = db::room_left(tx, parent.owner_id)
        .await?
        .unwrap_or(state.default_quota_bytes);
    let replaced = match db::child(tx, parent.id, name).await? {
        Some(existing) => existing.size + versions::held_by(tx, existing.id).await?,
        None => 0,
    };
    Ok(u64::try_from(left.saturating_add(replaced)).unwrap_or(0))
}

pub async fn receive(
    state: &AppState,
    headers: &HeaderMap,
    body: Body,
    room: u64,
) -> Result<Received, ApiError> {
    if declared_length(headers).is_some_and(|declared| declared > room) {
        return Err(ApiError::QuotaExceeded);
    }

    let written = state
        .blobs
        .write(storage::upload_within(body.into_data_stream(), room))
        .await
        .map_err(|error| match error {
            StorageError::OverLimit => ApiError::QuotaExceeded,
            other => other.into(),
        })?;
    let size = i64::try_from(written.size).map_err(|_| ApiError::QuotaExceeded)?;
    db::register_blob(&state.db, written.hash, size).await?;
    Ok(Received { written, size })
}

fn declared_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}
