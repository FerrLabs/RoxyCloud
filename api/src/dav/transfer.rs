use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use percent_encoding::percent_decode_str;

use super::PREFIX;
use super::auth::DavCaller;
use super::path_of;
use super::root_of;
use crate::db;
use crate::error::ApiError;
use crate::state::AppState;
use crate::trash;
use roxycloud_core::name::{NodeName, parse_path};

pub(super) async fn run(
    state: AppState,
    caller: DavCaller,
    request: Request,
) -> Result<Response, ApiError> {
    if !caller.0.may_write() {
        return Err(ApiError::Forbidden);
    }

    let moving = request.method().as_str() == "MOVE";
    let from = path_of(request.uri())?;
    if from.is_empty() {
        return Ok(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }

    let to = destination(request.headers())?;
    let Some((name, parents)) = to.split_last() else {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    };
    let overwrite = overwrite(request.headers());

    let owner = caller.0.id;
    let root = root_of(&state, owner).await?;
    let mut tx = state.db.begin().await?;

    let source = db::resolve(&mut tx, &root, &from).await?;
    let parent = match db::resolve(&mut tx, &root, parents).await {
        Ok(parent) => parent,
        Err(ApiError::NotFound) => return Ok(StatusCode::CONFLICT.into_response()),
        Err(other) => return Err(other),
    };

    let occupant = db::child(&mut tx, parent.id, name).await?;
    if occupant.is_some() && !overwrite {
        return Ok(StatusCode::PRECONDITION_FAILED.into_response());
    }
    let replaced = occupant.is_some();
    if let Some(occupant) = occupant {
        if occupant.id == source.id {
            return Ok(StatusCode::FORBIDDEN.into_response());
        }
        trash::send(&mut tx, &occupant).await?;
    }

    if moving {
        db::rename(&mut tx, &source, &parent, name).await?;
    } else {
        db::copy_tree(&mut tx, owner, &source, &parent, name).await?;
    }
    tx.commit().await?;

    Ok(if replaced {
        StatusCode::NO_CONTENT.into_response()
    } else {
        StatusCode::CREATED.into_response()
    })
}

/// `Destination` is a full URL more often than not, and clients disagree about whether it carries a
/// trailing slash, so only the path below the prefix is taken from it.
fn destination(headers: &HeaderMap) -> Result<Vec<NodeName>, ApiError> {
    let raw = headers
        .get("destination")
        .and_then(|value| value.to_str().ok())
        .ok_or(ApiError::WrongKind {
            expected: "Destination header",
        })?;

    let path = match raw.parse::<Uri>() {
        Ok(uri) => uri.path().to_owned(),
        Err(_) => raw.to_owned(),
    };
    let below = path.strip_prefix(PREFIX).ok_or(ApiError::WrongKind {
        expected: "Destination under /dav",
    })?;
    let decoded = percent_decode_str(below)
        .decode_utf8()
        .map_err(|_| ApiError::WrongKind {
            expected: "Destination in valid UTF-8",
        })?;

    parse_path(&decoded).map_err(Into::into)
}

fn overwrite(headers: &HeaderMap) -> bool {
    headers
        .get("overwrite")
        .and_then(|value| value.to_str().ok())
        .is_none_or(|value| !value.trim().eq_ignore_ascii_case("F"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).expect("a valid header"));
        }
        map
    }

    fn names(path: &[NodeName]) -> Vec<&str> {
        path.iter().map(NodeName::as_str).collect()
    }

    #[test]
    fn a_full_url_destination_is_reduced_to_its_path() {
        let map = headers(&[("destination", "https://files.example.com/dav/photos/x.jpg")]);
        assert_eq!(names(&destination(&map).unwrap()), ["photos", "x.jpg"]);
    }

    #[test]
    fn a_bare_path_destination_works_the_same() {
        let map = headers(&[("destination", "/dav/photos/x.jpg")]);
        assert_eq!(names(&destination(&map).unwrap()), ["photos", "x.jpg"]);
    }

    #[test]
    fn an_encoded_destination_names_what_it_stands_for() {
        let map = headers(&[("destination", "/dav/my%20docs/a%23b.txt")]);
        assert_eq!(names(&destination(&map).unwrap()), ["my docs", "a#b.txt"]);
    }

    #[test]
    fn a_trailing_slash_on_a_collection_is_not_a_nameless_segment() {
        let map = headers(&[("destination", "/dav/photos/summer/")]);
        assert_eq!(names(&destination(&map).unwrap()), ["photos", "summer"]);
    }

    #[test]
    fn a_destination_outside_the_prefix_is_refused() {
        let map = headers(&[("destination", "/v1/files/x.jpg")]);
        assert!(destination(&map).is_err());
    }

    #[test]
    fn a_destination_that_climbs_out_is_refused() {
        let map = headers(&[("destination", "/dav/../../etc/passwd")]);
        assert!(destination(&map).is_err());
    }

    #[test]
    fn overwrite_is_on_unless_a_client_says_otherwise() {
        assert!(overwrite(&HeaderMap::new()));
        assert!(overwrite(&headers(&[("overwrite", "T")])));
        assert!(!overwrite(&headers(&[("overwrite", "F")])));
        assert!(!overwrite(&headers(&[("overwrite", "f")])));
    }
}
