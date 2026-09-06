use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{Caller, Writer};
use crate::db;
use crate::error::ApiError;
use crate::shares::{self, Minted, Share};
use crate::state::AppState;
use roxycloud_core::name::{NodeName, parse_path};
use roxycloud_core::node::{Node, NodeKind};

/// A person choosing the password types it, so it cannot ride in the URL where it would land in
/// proxy logs and browser history alongside the token it protects.
pub const PASSWORD_HEADER: &str = "x-share-password";

#[derive(Deserialize)]
pub struct NewShare {
    path: String,
    expires_at: Option<DateTime<Utc>>,
    password: Option<String>,
}

/// What an anonymous visitor is told about a node: enough to show a listing and start a download,
/// and nothing that identifies the account behind the link or names a row elsewhere in the API.
#[derive(Serialize)]
pub struct Entry {
    name: String,
    kind: NodeKind,
    size: i64,
    updated_at: DateTime<Utc>,
}

impl From<Node> for Entry {
    fn from(node: Node) -> Self {
        Self {
            name: node.name,
            kind: node.kind,
            size: node.size,
            updated_at: node.updated_at,
        }
    }
}

#[derive(Serialize)]
pub struct Linked {
    entry: Entry,
    children: Vec<Entry>,
}

pub async fn create(
    State(state): State<AppState>,
    caller: Writer,
    Json(request): Json<NewShare>,
) -> Result<(StatusCode, Json<Minted>), ApiError> {
    let segments = parse_path(&request.path)?;
    if segments.is_empty() {
        return Err(ApiError::WrongKind {
            expected: "path below the root",
        });
    }

    let mut tx = state.db.begin().await?;
    let root = db::ensure_root(&mut tx, caller.user_id(), state.default_quota_bytes).await?;
    let node = db::resolve(&mut tx, &root, &segments).await?;
    let minted = shares::mint(
        &mut tx,
        caller.user_id(),
        &node,
        request.expires_at,
        request.password.as_deref(),
    )
    .await?;
    tx.commit().await?;

    Ok((StatusCode::CREATED, Json(minted)))
}

pub async fn list(
    State(state): State<AppState>,
    caller: Caller,
) -> Result<Json<Vec<Share>>, ApiError> {
    Ok(Json(shares::list(&state.db, caller.user_id()).await?))
}

pub async fn revoke(
    State(state): State<AppState>,
    caller: Caller,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    shares::revoke(&state.db, caller.user_id(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn open(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let node = resolve(&state, &token, &[], &headers).await?;
    listing(&state, node).await
}

pub async fn open_at(
    State(state): State<AppState>,
    Path((token, path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let node = resolve(&state, &token, &parse_path(&path)?, &headers).await?;
    listing(&state, node).await
}

pub async fn download(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let node = resolve(&state, &token, &[], &headers).await?;
    contents(&state, &node).await
}

pub async fn download_at(
    State(state): State<AppState>,
    Path((token, path)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let node = resolve(&state, &token, &parse_path(&path)?, &headers).await?;
    contents(&state, &node).await
}

/// A public URL is stable and carries no credential, so a shared cache left to its own heuristics
/// would keep serving a link after it was revoked, and would hand a body fetched with the right
/// password to the next visitor who gives none.
fn never_cached() -> [(HeaderName, HeaderValue); 1] {
    [(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))]
}

/// The one way in. A token names the node it was minted for, and a path walks down from there by
/// following children, so no path a visitor can write reaches a node the link does not cover.
async fn resolve(
    state: &AppState,
    token: &str,
    below: &[NodeName],
    headers: &HeaderMap,
) -> Result<Node, ApiError> {
    // A header value is opaque bytes, and `to_str` accepts visible ASCII alone, so asking for a
    // string that way would refuse every password carrying an accent.
    let presented = headers
        .get(PASSWORD_HEADER)
        .and_then(|value| std::str::from_utf8(value.as_bytes()).ok());

    let shared = shares::open(&state.db, token, presented).await?;
    if below.is_empty() {
        return Ok(shared);
    }

    let mut tx = state.db.begin().await?;
    let node = db::resolve(&mut tx, &shared, below).await?;
    tx.commit().await?;
    Ok(node)
}

async fn listing(state: &AppState, node: Node) -> Result<Response, ApiError> {
    let children = if node.kind == NodeKind::Directory {
        db::list_children(&state.db, node.id)
            .await?
            .into_iter()
            .map(Entry::from)
            .collect()
    } else {
        Vec::new()
    };

    Ok((
        never_cached(),
        Json(Linked {
            entry: node.into(),
            children,
        }),
    )
        .into_response())
}

async fn contents(state: &AppState, node: &Node) -> Result<Response, ApiError> {
    let mut response = super::files::bytes_of(state, node).await?;
    response.headers_mut().extend(never_cached());
    Ok(response)
}
