use axum::http::{HeaderMap, header};
use stashden_core::node::Node;

use crate::error::ApiError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Precondition {
    Unconditional,
    Exists,
    Absent,
    Matches(Vec<String>),
}

impl Precondition {
    #[must_use]
    pub fn from_headers(headers: &HeaderMap) -> Self {
        if let Some(wanted) = text(headers, header::IF_MATCH) {
            return if wanted == "*" {
                Self::Exists
            } else {
                Self::Matches(
                    wanted
                        .split(',')
                        .map(str::trim)
                        .filter(|tag| !tag.is_empty())
                        .map(str::to_owned)
                        .collect(),
                )
            };
        }
        match text(headers, header::IF_NONE_MATCH) {
            Some("*") => Self::Absent,
            _ => Self::Unconditional,
        }
    }

    pub fn check(&self, existing: Option<&Node>) -> Result<(), ApiError> {
        let holds = match self {
            Self::Unconditional => true,
            Self::Exists => existing.is_some(),
            Self::Absent => existing.is_none(),
            Self::Matches(tags) => existing.is_some_and(|node| tags.contains(&node.etag)),
        };
        if holds {
            Ok(())
        } else {
            Err(ApiError::PreconditionFailed)
        }
    }
}

fn text(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use chrono::Utc;
    use stashden_core::node::NodeKind;
    use uuid::Uuid;

    fn headers(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(name.clone(), HeaderValue::from_str(value).expect("ascii"));
        }
        map
    }

    fn file(etag: &str) -> Node {
        Node {
            id: Uuid::nil(),
            owner_id: Uuid::nil(),
            parent_id: None,
            name: "a.txt".to_owned(),
            kind: NodeKind::File,
            size: 1,
            blob_hash: None,
            etag: etag.to_owned(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
        }
    }

    #[test]
    fn no_header_lets_any_write_through() {
        let precondition = Precondition::from_headers(&HeaderMap::new());
        assert!(precondition.check(None).is_ok());
        assert!(precondition.check(Some(&file("\"a\""))).is_ok());
    }

    #[test]
    fn if_match_holds_only_for_the_etag_it_names() {
        let precondition = Precondition::from_headers(&headers(&[(header::IF_MATCH, "\"a\"")]));
        assert!(precondition.check(Some(&file("\"a\""))).is_ok());
        assert!(precondition.check(Some(&file("\"b\""))).is_err());
        assert!(
            precondition.check(None).is_err(),
            "a file deleted since the client saw it is a change too"
        );
    }

    #[test]
    fn if_match_accepts_any_tag_in_a_list() {
        let precondition =
            Precondition::from_headers(&headers(&[(header::IF_MATCH, "\"a\", \"b\"")]));
        assert!(precondition.check(Some(&file("\"b\""))).is_ok());
    }

    #[test]
    fn a_weak_tag_never_matches_a_strong_one() {
        let precondition = Precondition::from_headers(&headers(&[(header::IF_MATCH, "W/\"a\"")]));
        assert!(precondition.check(Some(&file("\"a\""))).is_err());
    }

    #[test]
    fn if_none_match_star_refuses_to_replace_anything() {
        let precondition = Precondition::from_headers(&headers(&[(header::IF_NONE_MATCH, "*")]));
        assert!(precondition.check(None).is_ok());
        assert!(precondition.check(Some(&file("\"a\""))).is_err());
    }

    #[test]
    fn if_match_star_needs_something_there() {
        let precondition = Precondition::from_headers(&headers(&[(header::IF_MATCH, "*")]));
        assert!(precondition.check(None).is_err());
        assert!(precondition.check(Some(&file("\"a\""))).is_ok());
    }
}
