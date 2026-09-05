pub mod auth;
mod methods;
mod propfind;
mod transfer;
mod xml;

use axum::Router;
use axum::routing::any;
use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};
use uuid::Uuid;

use crate::db;
use crate::error::ApiError;
use crate::state::AppState;
use roxycloud_core::name::{NodeName, parse_path};
use roxycloud_core::node::Node;

const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}');

pub const PREFIX: &str = "/dav";

/// Every method arrives at one handler because `MethodFilter` knows nothing of PROPFIND, MKCOL,
/// COPY or MOVE.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(PREFIX, any(methods::dispatch))
        .route("/dav/", any(methods::dispatch))
        .route("/dav/{*path}", any(methods::dispatch))
}

fn path_of(uri: &axum::http::Uri) -> Result<Vec<NodeName>, ApiError> {
    let raw = uri.path().strip_prefix(PREFIX).unwrap_or("");
    let decoded = percent_decode_str(raw)
        .decode_utf8()
        .map_err(|_| ApiError::WrongKind {
            expected: "path in valid UTF-8",
        })?;
    parse_path(&decoded).map_err(Into::into)
}

async fn root_of(state: &AppState, owner: Uuid) -> Result<Node, ApiError> {
    let mut tx = state.db.begin().await?;
    let root = db::ensure_root(&mut tx, owner, state.default_quota_bytes).await?;
    tx.commit().await?;
    Ok(root)
}

/// The href in a multistatus has to match the request path a client sent, encoded the same way.
fn href(path: &[NodeName], collection: bool) -> String {
    let mut out = String::from(PREFIX);
    for segment in path {
        out.push('/');
        out.push_str(&utf8_percent_encode(segment.as_str(), PATH_SEGMENT).to_string());
    }
    if collection && !out.ends_with('/') {
        out.push('/');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(raw: &str) -> Vec<NodeName> {
        parse_path(raw).expect("a valid path")
    }

    #[test]
    fn a_collection_href_ends_in_a_slash_because_clients_join_onto_it() {
        assert_eq!(href(&names("photos"), true), "/dav/photos/");
        assert_eq!(href(&[], true), "/dav/");
    }

    #[test]
    fn a_file_href_does_not() {
        assert_eq!(href(&names("photos/x.jpg"), false), "/dav/photos/x.jpg");
    }

    #[test]
    fn a_space_or_a_hash_in_a_name_is_encoded_rather_than_ending_the_path() {
        assert_eq!(
            href(&names("my docs/a#b.txt"), false),
            "/dav/my%20docs/a%23b.txt"
        );
    }

    #[test]
    fn an_encoded_request_path_comes_back_as_the_name_it_stands_for() {
        let uri: axum::http::Uri = "/dav/my%20docs/a%23b.txt".parse().expect("a valid uri");
        let path = path_of(&uri).expect("a valid path");
        assert_eq!(
            path.iter().map(NodeName::as_str).collect::<Vec<_>>(),
            ["my docs", "a#b.txt"]
        );
    }

    #[test]
    fn the_collection_at_the_root_is_an_empty_path() {
        for raw in ["/dav", "/dav/"] {
            let uri: axum::http::Uri = raw.parse().expect("a valid uri");
            assert!(path_of(&uri).expect("a valid path").is_empty());
        }
    }

    #[test]
    fn a_path_that_climbs_out_is_refused_before_it_reaches_the_tree() {
        let uri: axum::http::Uri = "/dav/photos/../../etc".parse().expect("a valid uri");
        assert!(path_of(&uri).is_err());
    }
}
