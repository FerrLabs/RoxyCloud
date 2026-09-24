use roxycloud_client::RemoteError;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Failure {
    status: Option<u16>,
    message: String,
}

impl From<RemoteError> for Failure {
    fn from(error: RemoteError) -> Self {
        Self {
            status: error.status().map(|status| status.as_u16()),
            message: error.to_string(),
        }
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            status: None,
            message,
        }
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        Self {
            status: None,
            message: message.to_owned(),
        }
    }
}
