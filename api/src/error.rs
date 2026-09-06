use axum::Json;
use axum::http::StatusCode;
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
    #[error("expected a {expected}")]
    WrongKind { expected: &'static str },
    #[error("storage failure")]
    Storage(#[from] StorageError),
    #[error("database failure")]
    Database(#[from] sqlx::Error),
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
            Self::Unauthenticated | Self::InvalidCredentials => StatusCode::UNAUTHORIZED,
            Self::WeakPassword(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::NotFound | Self::Storage(StorageError::NotFound(_)) => StatusCode::NOT_FOUND,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::Conflict(_) | Self::MoveIntoSelf => StatusCode::CONFLICT,
            Self::Locked(_) => StatusCode::LOCKED,
            Self::InvalidPath(_) | Self::InvalidEmail(_) | Self::WrongKind { .. } => {
                StatusCode::BAD_REQUEST
            }
            Self::QuotaExceeded => StatusCode::INSUFFICIENT_STORAGE,
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
        (status, Json(json!({ "error": body }))).into_response()
    }
}
