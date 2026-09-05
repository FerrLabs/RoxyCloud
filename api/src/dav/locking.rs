use std::fmt::Write;

use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use quick_xml::NsReader;
use quick_xml::events::Event;

use super::auth::DavCaller;
use super::locks::{self, Lock};
use super::xml::escape;
use super::{href, path_of, root_of};
use crate::db;
use crate::error::ApiError;
use crate::state::AppState;
use roxycloud_core::name::NodeName;
use roxycloud_core::node::{Node, NodeKind};

pub(super) async fn lock(
    state: AppState,
    caller: DavCaller,
    request: Request,
    body: &[u8],
) -> Result<Response, ApiError> {
    if !caller.0.may_write() {
        return Err(ApiError::Forbidden);
    }

    let path = path_of(request.uri())?;
    let headers = request.headers().clone();
    let seconds = locks::requested_seconds(text(&headers, "timeout"));
    let deep = !matches!(text(&headers, "depth"), Some("0"));

    let owner = caller.0.id;
    let root = root_of(&state, owner).await?;
    let mut tx = state.db.begin().await?;

    // An empty body is a refresh of a lock the client already holds, named in the If header.
    if body.iter().all(u8::is_ascii_whitespace) {
        let node = db::resolve(&mut tx, &root, &path).await?;
        for token in locks::submitted_tokens(text(&headers, "if")) {
            if let Some(refreshed) = locks::refresh(&mut tx, &token, &node, seconds).await? {
                tx.commit().await?;
                return Ok(granted(&refreshed, &path, node.kind, StatusCode::OK));
            }
        }
        return Err(ApiError::WrongKind {
            expected: "lockinfo body, or the If header naming a lock to refresh",
        });
    }

    // A client locks the file it is about to create, so LOCK on nothing creates an empty one.
    let (node, created) = match db::resolve(&mut tx, &root, &path).await {
        Ok(node) => (node, false),
        Err(ApiError::NotFound) => (
            empty_file(&state, &mut tx, owner, &root, &path).await?,
            true,
        ),
        Err(other) => return Err(other),
    };

    locks::allows(
        &mut tx,
        &node,
        &locks::submitted_tokens(text(&headers, "if")),
    )
    .await?;

    let taken = locks::take(
        &mut tx,
        &node,
        owner,
        holder(body).as_deref(),
        deep,
        seconds,
    )
    .await?;
    tx.commit().await?;

    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok(granted(&taken, &path, node.kind, status))
}

pub(super) async fn unlock(
    state: AppState,
    caller: DavCaller,
    request: Request,
) -> Result<Response, ApiError> {
    if !caller.0.may_write() {
        return Err(ApiError::Forbidden);
    }

    let path = path_of(request.uri())?;
    let Some(token) = locks::lock_token(text(request.headers(), "lock-token")) else {
        return Err(ApiError::WrongKind {
            expected: "Lock-Token header",
        });
    };

    let root = root_of(&state, caller.0.id).await?;
    let mut tx = state.db.begin().await?;
    let node = db::resolve(&mut tx, &root, &path).await?;
    tx.commit().await?;

    if locks::release(&state.db, &node, &token).await? {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    // The token names no lock on this resource, which is a conflict rather than a silent success:
    // a client told otherwise believes it released something.
    Ok(StatusCode::CONFLICT.into_response())
}

async fn empty_file(
    state: &AppState,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner: uuid::Uuid,
    root: &Node,
    path: &[NodeName],
) -> Result<Node, ApiError> {
    let Some((name, parents)) = path.split_last() else {
        return Err(ApiError::WrongKind {
            expected: "path below the root",
        });
    };

    let parent = match db::resolve(tx, root, parents).await {
        Ok(parent) => parent,
        Err(ApiError::NotFound) => return Err(ApiError::Conflict(name.to_string())),
        Err(other) => return Err(other),
    };

    let written = state
        .blobs
        .write(futures::stream::iter([Ok::<_, std::io::Error>(
            bytes::Bytes::new(),
        )]))
        .await?;
    db::register_blob(&state.db, written.hash, 0).await?;
    let node = db::put_file(tx, owner, &parent, name, written.hash, 0).await?;
    state.blobs.settle(&written).await?;

    Ok(node)
}

fn granted(lock: &Lock, path: &[NodeName], kind: NodeKind, status: StatusCode) -> Response {
    let mut document = String::from(concat!(
        r#"<?xml version="1.0" encoding="utf-8"?>"#,
        r#"<D:prop xmlns:D="DAV:"><D:lockdiscovery>"#
    ));
    document.push_str(&active(lock, path, kind));
    document.push_str("</D:lockdiscovery></D:prop>");

    (
        status,
        [
            (
                header::CONTENT_TYPE,
                "application/xml; charset=utf-8".to_owned(),
            ),
            (
                header::HeaderName::from_static("lock-token"),
                format!("<{}>", lock.token),
            ),
        ],
        document,
    )
        .into_response()
}

pub(super) fn active(lock: &Lock, path: &[NodeName], kind: NodeKind) -> String {
    let mut out = String::from(
        "<D:activelock><D:locktype><D:write/></D:locktype>\
         <D:lockscope><D:exclusive/></D:lockscope>",
    );
    let _ = write!(
        out,
        "<D:depth>{}</D:depth>",
        if lock.deep { "infinity" } else { "0" }
    );
    if let Some(holder) = &lock.holder {
        let _ = write!(out, "<D:owner>{}</D:owner>", escape(holder));
    }
    let _ = write!(
        out,
        "<D:timeout>{}</D:timeout>\
         <D:locktoken><D:href>{}</D:href></D:locktoken>\
         <D:lockroot><D:href>{}</D:href></D:lockroot></D:activelock>",
        locks::timeout_header(lock),
        escape(&lock.token),
        escape(&href(path, kind == NodeKind::Directory))
    );
    out
}

/// The `<owner>` a client sends is opaque: it is shown back in `lockdiscovery` so a person can see
/// who holds the file, and nothing here interprets it.
fn holder(body: &[u8]) -> Option<String> {
    let mut reader = NsReader::from_reader(body);
    reader.config_mut().trim_text(true);

    let mut inside = false;
    let mut text = String::new();
    loop {
        match reader.read_resolved_event() {
            Ok((_, Event::Start(element))) if element.local_name().into_inner() == b"owner" => {
                inside = true;
            }
            Ok((_, Event::End(element))) if element.local_name().into_inner() == b"owner" => break,
            Ok((_, Event::Text(raw))) if inside => {
                text.push_str(&String::from_utf8_lossy(&raw));
            }
            Ok((_, Event::Eof)) => break,
            Ok(_) => {}
            Err(_) => return None,
        }
    }

    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.chars().take(1000).collect())
}

fn text<'h>(headers: &'h HeaderMap, name: &str) -> Option<&'h str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_owner_a_client_sends_is_kept_as_written() {
        let holder = holder(
            br#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope>
                <D:locktype><D:write/></D:locktype><D:owner>bryan on the laptop</D:owner>
                </D:lockinfo>"#,
        );
        assert_eq!(holder.as_deref(), Some("bryan on the laptop"));
    }

    #[test]
    fn a_lockinfo_without_an_owner_names_nobody() {
        let holder = holder(
            br#"<D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope>
                <D:locktype><D:write/></D:locktype></D:lockinfo>"#,
        );
        assert_eq!(holder, None);
    }

    #[test]
    fn an_owner_that_is_markup_cannot_break_the_answer() {
        let lock = Lock {
            token: "opaquelocktoken:abc".to_owned(),
            node_id: uuid::Uuid::nil(),
            owner_id: uuid::Uuid::nil(),
            holder: Some("</D:owner><script>".to_owned()),
            deep: false,
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(60),
        };

        let rendered = active(&lock, &[], NodeKind::File);
        assert!(rendered.contains("&lt;/D:owner&gt;"), "{rendered}");
        assert!(!rendered.contains("<script>"), "{rendered}");
    }
}
