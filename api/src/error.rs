use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::storage::StorageError;
use roxycloud_core::name::InvalidNodeName;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthenticated")]
    Unauthenticated,
    #[error("wrong email or password")]
    InvalidCredentials,
    #[error(transparent)]
    WeakPassword(#[from] crate::password::WeakPassword),
    #[error("internal credential failure")]
    Credential,
    #[error("not found")]
    NotFound,
    #[error("this link needs its password")]
    SharePassword,
    #[error("too many attempts, try again in {seconds} seconds")]
    TooManyAttempts { seconds: i64 },
    #[error("this account may not write")]
    Forbidden,
    #[error("{0} already exists")]
    Conflict(String),
    #[error("a directory cannot be moved inside itself")]
    MoveIntoSelf,
    #[error("{0} is locked")]
    Locked(String),
    #[error("invalid path: {0}")]
    InvalidPath(#[from] InvalidNodeName),
    #[error(transparent)]
    InvalidEmail(#[from] roxycloud_core::user::InvalidEmail),
    #[error("quota exceeded")]
    QuotaExceeded,
    #[error("this upload is at {expected} bytes")]
    OffsetMismatch { expected: i64 },
    #[error("this upload has {received} of {expected} bytes")]
    Incomplete { received: i64, expected: i64 },
    #[error("this account already has {most} uploads open")]
    TooManySessions { most: i64 },
    #[error("expected a {expected}")]
    WrongKind { expected: &'static str },
    #[error("storage failure")]
    Storage(#[from] StorageError),
    #[error("database failure")]
    Database(#[from] sqlx::Error),
}

/// The staging area an upload in flight writes to is storage, so a disk that refuses is the same
/// kind of failure as a blob store that refuses.
impl From<std::io::Error> for ApiError {
    fn from(err: std::io::Error) -> Self {
        Self::Storage(StorageError::Io(err))
    }
}

impl From<crate::password::HashFailed> for ApiError {
    fn from(_: crate::password::HashFailed) -> Self {
        Self::Credential
    }
}

impl From<crate::auth::SignFailed> for ApiError {
    fn from(_: crate::auth::SignFailed) -> Self {
        Self::Credential
    }
}

impl ApiError {
    fn status(&self) -> StatusCode {
        match self {
            Self::Unauthenticated | Self::InvalidCredentials | Self::SharePassword => {
                StatusCode::UNAUTHORIZED
            }
            Self::WeakPassword(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::NotFound | Self::Storage(StorageError::NotFound(_)) => StatusCode::NOT_FOUND,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::Conflict(_) | Self::MoveIntoSelf | Self::OffsetMismatch { .. } => {
                StatusCode::CONFLICT
            }
            Self::Incomplete { .. } => StatusCode::BAD_REQUEST,
            Self::Locked(_) => StatusCode::LOCKED,
            Self::InvalidPath(_) | Self::InvalidEmail(_) | Self::WrongKind { .. } => {
                StatusCode::BAD_REQUEST
            }
            Self::QuotaExceeded => StatusCode::INSUFFICIENT_STORAGE,
            Self::TooManySessions { .. } | Self::TooManyAttempts { .. } => {
                StatusCode::TOO_MANY_REQUESTS
            }
            Self::Credential | Self::Storage(_) | Self::Database(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        if status.is_server_error() {
            tracing::error!(error = ?self, "request failed");
        }
        let body = if status.is_server_error() {
            "internal error".to_owned()
        } else {
            self.to_string()
        };
        let mut response = (status, Json(json!({ "error": body }))).into_response();

        // A client that lost the connection knows what it sent, not what arrived, so the offset to
        // resume from travels with the refusal rather than needing another round trip.
        if let Self::OffsetMismatch { expected } = self {
            response
                .headers_mut()
                .insert("upload-offset", HeaderValue::from(expected));
        }

        // A limiter a client cannot cooperate with is one it answers by retrying immediately.
        if let Self::TooManyAttempts { seconds } = self
            && let Ok(after) = HeaderValue::from_str(&seconds.to_string())
        {
            response.headers_mut().insert(header::RETRY_AFTER, after);
        }
        response
    }
}
