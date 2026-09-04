use std::path::Path;

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use reqwest::{Client, StatusCode};
use roxycloud_core::name::{InvalidNodeName, parse_path};
use roxycloud_core::node::Node;
use serde::Deserialize;
use uuid::Uuid;

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
    .add(b'}')
    .add(b'/');

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("invalid remote path: {0}")]
    Path(#[from] InvalidNodeName),
    #[error("the server rejected the credentials")]
    Unauthenticated,
    #[error("{0} is not on the server")]
    NotFound(String),
    #[error("{0} clashes with something already on the server")]
    Conflict(String),
    #[error("the server answered {0}")]
    Status(StatusCode),
    #[error("talking to the server failed")]
    Transport(#[from] reqwest::Error),
    #[error("reading or writing {path}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl RemoteError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

pub struct Remote {
    base: String,
    token: String,
    http: Client,
}

#[derive(Debug, Deserialize)]
pub struct Session {
    pub token: String,
    pub expires_in: i64,
}

impl Remote {
    pub async fn login(
        base_url: &str,
        email: &str,
        password: &str,
    ) -> Result<(Self, Session), RemoteError> {
        let base = base_url.trim_end_matches('/').to_owned();
        let http = Client::builder().build()?;
        let response = http
            .post(format!("{base}/v1/auth/login"))
            .json(&serde_json::json!({ "email": email, "password": password }))
            .send()
            .await?;
        check(response.status(), email)?;

        let session: Session = response.json().await?;
        let remote = Self {
            base,
            token: session.token.clone(),
            http,
        };
        Ok((remote, session))
    }

    pub fn new(base_url: &str, token: impl Into<String>) -> Result<Self, RemoteError> {
        Ok(Self {
            base: base_url.trim_end_matches('/').to_owned(),
            token: token.into(),
            http: Client::builder().build()?,
        })
    }

    pub(crate) fn http(&self) -> &Client {
        &self.http
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    pub(crate) fn base(&self) -> &str {
        &self.base
    }

    pub fn endpoint(&self, collection: &str, path: &str) -> Result<String, RemoteError> {
        let segments = parse_path(path)?;
        if segments.is_empty() {
            return Ok(format!("{}/v1/{collection}", self.base));
        }
        let encoded = segments
            .iter()
            .map(|segment| utf8_percent_encode(segment.as_str(), PATH_SEGMENT).to_string())
            .collect::<Vec<_>>()
            .join("/");
        Ok(format!("{}/v1/{collection}/{encoded}", self.base))
    }

    pub async fn list(&self, path: &str) -> Result<Vec<Node>, RemoteError> {
        let url = self.endpoint("folders", path)?;
        let response = self.http.get(&url).bearer_auth(&self.token).send().await?;
        check(response.status(), path)?;
        Ok(response.json().await?)
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<Node, RemoteError> {
        parse_path(from)?;
        parse_path(to)?;
        let response = self
            .http
            .post(format!("{}/v1/move", self.base))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({ "from": from, "to": to }))
            .send()
            .await?;
        if response.status() == StatusCode::CONFLICT {
            return Err(RemoteError::Conflict(to.to_owned()));
        }
        check(response.status(), &format!("{from} or {to}"))?;
        Ok(response.json().await?)
    }

    pub async fn trash(&self) -> Result<Vec<Node>, RemoteError> {
        let response = self
            .http
            .get(format!("{}/v1/trash", self.base))
            .bearer_auth(&self.token)
            .send()
            .await?;
        check(response.status(), "the trash")?;
        Ok(response.json().await?)
    }

    pub async fn restore(&self, id: Uuid) -> Result<Node, RemoteError> {
        let response = self
            .http
            .post(format!("{}/v1/trash/{id}/restore", self.base))
            .bearer_auth(&self.token)
            .send()
            .await?;
        if response.status() == StatusCode::CONFLICT {
            return Err(RemoteError::Conflict(id.to_string()));
        }
        check(response.status(), &id.to_string())?;
        Ok(response.json().await?)
    }

    pub async fn purge(&self, id: Uuid) -> Result<(), RemoteError> {
        let response = self
            .http
            .delete(format!("{}/v1/trash/{id}", self.base))
            .bearer_auth(&self.token)
            .send()
            .await?;
        check(response.status(), &id.to_string())
    }

    pub async fn delete(&self, path: &str) -> Result<(), RemoteError> {
        let url = self.endpoint("files", path)?;
        let response = self
            .http
            .delete(&url)
            .bearer_auth(&self.token)
            .send()
            .await?;
        check(response.status(), path)
    }
}

pub(crate) fn check(status: StatusCode, path: &str) -> Result<(), RemoteError> {
    match status {
        s if s.is_success() => Ok(()),
        StatusCode::UNAUTHORIZED => Err(RemoteError::Unauthenticated),
        StatusCode::NOT_FOUND => Err(RemoteError::NotFound(path.to_owned())),
        StatusCode::CONFLICT => Err(RemoteError::Conflict(path.to_owned())),
        other => Err(RemoteError::Status(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote() -> Remote {
        Remote::new("https://api.roxycloud.io/", "token").expect("client builds")
    }

    #[test]
    fn trailing_slash_on_the_base_url_does_not_double_up() {
        assert_eq!(
            remote().endpoint("files", "a").unwrap(),
            "https://api.roxycloud.io/v1/files/a"
        );
    }

    #[test]
    fn separators_between_segments_stay_literal() {
        assert_eq!(
            remote().endpoint("files", "photos/summer/x.jpg").unwrap(),
            "https://api.roxycloud.io/v1/files/photos/summer/x.jpg"
        );
    }

    #[test]
    fn spaces_and_reserved_characters_are_encoded() {
        assert_eq!(
            remote().endpoint("files", "my docs/a#b?c.txt").unwrap(),
            "https://api.roxycloud.io/v1/files/my%20docs/a%23b%3Fc.txt"
        );
    }

    #[test]
    fn a_slash_inside_a_name_cannot_forge_a_path() {
        let sneaky = utf8_percent_encode("a/b", PATH_SEGMENT).to_string();
        assert_eq!(sneaky, "a%2Fb");
    }

    #[tokio::test]
    async fn a_traversing_destination_never_reaches_the_server() {
        assert!(matches!(
            remote().rename("a.txt", "../../etc/passwd").await,
            Err(RemoteError::Path(_))
        ));
    }

    #[test]
    fn traversal_is_refused_before_a_request_is_built() {
        assert!(matches!(
            remote().endpoint("files", "photos/../../etc/passwd"),
            Err(RemoteError::Path(_))
        ));
    }

    #[test]
    fn the_root_has_no_trailing_segment() {
        assert_eq!(
            remote().endpoint("folders", "/").unwrap(),
            "https://api.roxycloud.io/v1/folders"
        );
    }
}
