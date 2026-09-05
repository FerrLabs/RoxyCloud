use std::fmt::Write;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use super::auth::DavCaller;
use super::locks;
use super::xml::{self, MULTISTATUS_CLOSE, MULTISTATUS_OPEN, Quota};
use super::{href, locking, path_of, propfind, root_of, transfer};
use crate::db;
use crate::error::ApiError;
use crate::routes::files::never_rendered;
use crate::state::AppState;
use crate::trash;
use roxycloud_core::node::NodeKind;

pub(super) const ALLOWED: &str =
    "OPTIONS, PROPFIND, PROPPATCH, MKCOL, GET, HEAD, PUT, DELETE, COPY, MOVE, LOCK, UNLOCK";

const MAX_XML_BODY: usize = 1 << 20;

pub async fn dispatch(
    State(state): State<AppState>,
    caller: DavCaller,
    request: Request,
) -> Result<Response, ApiError> {
    let method = request.method().clone();
    match method.as_str() {
        "OPTIONS" => Ok(options()),
        "PROPFIND" => propfind(state, caller, request).await,
        "PROPPATCH" => proppatch(state, caller, request).await,
        "MKCOL" => mkcol(state, caller, request).await,
        "GET" | "HEAD" => read(state, caller, request, method == Method::HEAD).await,
        "PUT" => put(state, caller, request).await,
        "DELETE" => delete(state, caller, request).await,
        "COPY" | "MOVE" => transfer::run(state, caller, request).await,
        "LOCK" => {
            let headers = request.headers().clone();
            let uri = request.uri().clone();
            let body = body_of(request).await?;
            let mut rebuilt = Request::builder().method("LOCK").uri(uri);
            for (name, value) in &headers {
                rebuilt = rebuilt.header(name, value);
            }
            let rebuilt =
                rebuilt
                    .body(axum::body::Body::empty())
                    .map_err(|_| ApiError::WrongKind {
                        expected: "well formed LOCK request",
                    })?;
            locking::lock(state, caller, rebuilt, &body).await
        }
        "UNLOCK" => locking::unlock(state, caller, request).await,
        _ => Ok(refused()),
    }
}

fn options() -> Response {
    (
        StatusCode::NO_CONTENT,
        [
            (header::HeaderName::from_static("dav"), "1, 2"),
            (header::ALLOW, ALLOWED),
            (header::HeaderName::from_static("ms-author-via"), "DAV"),
        ],
    )
        .into_response()
}

fn refused() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [(header::ALLOW, ALLOWED)],
        (),
    )
        .into_response()
}

async fn body_of(request: Request) -> Result<bytes::Bytes, ApiError> {
    axum::body::to_bytes(request.into_body(), MAX_XML_BODY)
        .await
        .map_err(|_| ApiError::WrongKind {
            expected: "body under a megabyte",
        })
}

async fn propfind(
    state: AppState,
    caller: DavCaller,
    request: Request,
) -> Result<Response, ApiError> {
    let depth = request
        .headers()
        .get("depth")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("infinity")
        .trim()
        .to_owned();
    let path = path_of(request.uri())?;
    let body = body_of(request).await?;

    if depth.eq_ignore_ascii_case("infinity") {
        return Ok((
            StatusCode::FORBIDDEN,
            [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
            concat!(
                r#"<?xml version="1.0" encoding="utf-8"?>"#,
                r#"<D:error xmlns:D="DAV:"><D:propfind-finite-depth/></D:error>"#
            ),
        )
            .into_response());
    }

    let owner = caller.0.id;
    let root = root_of(&state, owner).await?;
    let mut tx = state.db.begin().await?;
    let node = db::resolve(&mut tx, &root, &path).await?;

    let listed = if depth == "1" && node.kind == NodeKind::Directory {
        db::list_children(&state.db, node.id).await?
    } else {
        Vec::new()
    };

    let mut ids = vec![node.id];
    ids.extend(listed.iter().map(|below| below.id));
    let held = locks::for_nodes(&mut tx, &ids).await?;
    tx.commit().await?;

    let requested = propfind::parse(&body);
    let quota = quota_of(&state, owner).await?;
    let lock_on = |id: Uuid, path: &[roxycloud_core::name::NodeName], kind: NodeKind| {
        held.iter()
            .find(|lock| lock.node_id == id)
            .map(|lock| locking::active(lock, path, kind))
    };

    let mut document = String::from(MULTISTATUS_OPEN);
    document.push_str(&xml::response(
        &href(&path, node.kind == NodeKind::Directory),
        &node,
        &quota,
        &requested,
        lock_on(node.id, &path, node.kind).as_deref(),
    ));

    for below in listed {
        let mut path = path.clone();
        path.push(below.name.parse()?);
        document.push_str(&xml::response(
            &href(&path, below.kind == NodeKind::Directory),
            &below,
            &quota,
            &requested,
            lock_on(below.id, &path, below.kind).as_deref(),
        ));
    }
    document.push_str(MULTISTATUS_CLOSE);

    Ok(multistatus(document))
}

/// Nothing here stores dead properties. Answering 403 for each one is what the specification asks
/// for, and it beats a 200 that has a client believe the timestamp it set survived.
async fn proppatch(
    state: AppState,
    caller: DavCaller,
    request: Request,
) -> Result<Response, ApiError> {
    if !caller.0.may_write() {
        return Err(ApiError::Forbidden);
    }

    let path = path_of(request.uri())?;
    let submitted = locks::submitted_tokens(header_text(request.headers(), "if"));
    let body = body_of(request).await?;

    let root = root_of(&state, caller.0.id).await?;
    let mut tx = state.db.begin().await?;
    let node = db::resolve(&mut tx, &root, &path).await?;
    locks::allows(&mut tx, &node, &submitted).await?;
    tx.commit().await?;

    let requested = propfind::parse(&body);
    let mut refused = String::new();
    for property in &requested.properties {
        let _ = write!(refused, "<D:{}/>", property.name());
    }
    for property in &requested.unknown {
        refused.push_str(&xml::foreign(property));
    }

    let mut document = String::from(MULTISTATUS_OPEN);
    let _ = write!(
        document,
        "<D:response><D:href>{}</D:href><D:propstat><D:prop>{refused}</D:prop>\
         <D:status>HTTP/1.1 403 Forbidden</D:status></D:propstat></D:response>",
        xml::escape(&href(&path, node.kind == NodeKind::Directory))
    );
    document.push_str(MULTISTATUS_CLOSE);

    Ok(multistatus(document))
}

async fn mkcol(state: AppState, caller: DavCaller, request: Request) -> Result<Response, ApiError> {
    if !caller.0.may_write() {
        return Err(ApiError::Forbidden);
    }

    let path = path_of(request.uri())?;
    let submitted = locks::submitted_tokens(header_text(request.headers(), "if"));
    let body = body_of(request).await?;
    if !body.is_empty() {
        return Ok(StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response());
    }

    let Some((name, parents)) = path.split_last() else {
        return Ok(refused());
    };

    let owner = caller.0.id;
    let root = root_of(&state, owner).await?;
    let mut tx = state.db.begin().await?;
    let parent = match db::resolve(&mut tx, &root, parents).await {
        Ok(parent) => parent,
        Err(ApiError::NotFound) => return Ok(StatusCode::CONFLICT.into_response()),
        Err(other) => return Err(other),
    };
    if db::child(&mut tx, parent.id, name).await?.is_some() {
        return Ok(refused());
    }
    locks::allows(&mut tx, &parent, &submitted).await?;
    db::create_directories(&mut tx, owner, &parent, std::slice::from_ref(name)).await?;
    tx.commit().await?;

    Ok(StatusCode::CREATED.into_response())
}

async fn read(
    state: AppState,
    caller: DavCaller,
    request: Request,
    head_only: bool,
) -> Result<Response, ApiError> {
    let path = path_of(request.uri())?;
    let root = root_of(&state, caller.0.id).await?;
    let mut tx = state.db.begin().await?;
    let node = db::resolve(&mut tx, &root, &path).await?;
    tx.commit().await?;

    let (NodeKind::File, Some(hash)) = (node.kind, node.blob_hash) else {
        return Ok(refused());
    };

    let headers = [
        (
            header::ETAG,
            HeaderValue::from_str(&node.etag).map_err(|_| ApiError::WrongKind {
                expected: "printable etag",
            })?,
        ),
        (
            header::CONTENT_LENGTH,
            HeaderValue::from(u64::try_from(node.size).unwrap_or(0)),
        ),
    ];

    if head_only {
        return Ok((headers, never_rendered(), ()).into_response());
    }

    let file = state.blobs.read(hash).await?;
    Ok((
        headers,
        never_rendered(),
        Body::from_stream(ReaderStream::new(file)),
    )
        .into_response())
}

async fn put(state: AppState, caller: DavCaller, request: Request) -> Result<Response, ApiError> {
    if !caller.0.may_write() {
        return Err(ApiError::Forbidden);
    }

    let path = path_of(request.uri())?;
    let submitted = locks::submitted_tokens(header_text(request.headers(), "if"));
    let Some((name, parents)) = path.split_last() else {
        return Ok(refused());
    };

    let owner = caller.0.id;
    let root = root_of(&state, owner).await?;
    let written = state
        .blobs
        .write(request.into_body().into_data_stream())
        .await?;
    let size = i64::try_from(written.size).map_err(|_| ApiError::QuotaExceeded)?;
    db::register_blob(&state.db, written.hash, size).await?;

    let mut tx = state.db.begin().await?;
    // Unlike the REST upload, WebDAV does not invent the directories above a file.
    let parent = match db::resolve(&mut tx, &root, parents).await {
        Ok(parent) => parent,
        Err(ApiError::NotFound) => return Ok(StatusCode::CONFLICT.into_response()),
        Err(other) => return Err(other),
    };
    let existing = db::child(&mut tx, parent.id, name).await?;
    let existed = existing.is_some();
    match &existing {
        Some(node) => locks::allows(&mut tx, node, &submitted).await?,
        None => locks::allows(&mut tx, &parent, &submitted).await?,
    }
    let node = db::put_file(&mut tx, owner, &parent, name, written.hash, size).await?;
    tx.commit().await?;
    state.blobs.settle(&written).await?;

    let status = if existed {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::CREATED
    };
    Ok((
        status,
        [(
            header::ETAG,
            HeaderValue::from_str(&node.etag).map_err(|_| ApiError::WrongKind {
                expected: "printable etag",
            })?,
        )],
    )
        .into_response())
}

async fn delete(
    state: AppState,
    caller: DavCaller,
    request: Request,
) -> Result<Response, ApiError> {
    if !caller.0.may_write() {
        return Err(ApiError::Forbidden);
    }

    let path = path_of(request.uri())?;
    if path.is_empty() {
        return Ok(refused());
    }

    let submitted = locks::submitted_tokens(header_text(request.headers(), "if"));
    let root = root_of(&state, caller.0.id).await?;
    let mut tx = state.db.begin().await?;
    let node = db::resolve(&mut tx, &root, &path).await?;
    locks::allows(&mut tx, &node, &submitted).await?;
    locks::none_below(&mut tx, &node, &submitted).await?;
    if let Some((_, parents)) = path.split_last() {
        // Taking a member out of a collection is a change to the collection, which its own lock
        // governs even when that lock was taken with Depth 0.
        let parent = db::resolve(&mut tx, &root, parents).await?;
        locks::allows(&mut tx, &parent, &submitted).await?;
    }
    trash::send(&mut tx, &node).await?;
    tx.commit().await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}

pub(super) fn header_text<'h>(headers: &'h axum::http::HeaderMap, name: &str) -> Option<&'h str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
}

pub(super) async fn quota_of(state: &AppState, owner: Uuid) -> Result<Quota, ApiError> {
    let (used, max) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT bytes_used, bytes_max FROM quotas WHERE owner_id = $1",
    )
    .bind(owner)
    .fetch_one(&state.db)
    .await?;

    Ok(Quota {
        used,
        available: (max - used).max(0),
    })
}

pub(super) fn multistatus(document: String) -> Response {
    (
        StatusCode::MULTI_STATUS,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        document,
    )
        .into_response()
}
