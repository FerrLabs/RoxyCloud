use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct Share {
    pub id: Uuid,
    pub node_id: Uuid,
    pub name: String,
    pub has_password: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Minted {
    #[serde(flatten)]
    pub share: Share,
    /// The only time the token exists outside the link that will carry it.
    pub token: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct NewShare {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

impl std::fmt::Debug for NewShare {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NewShare")
            .field("path", &self.path)
            .field("expires_at", &self.expires_at)
            .field("password", &self.password.as_ref().map(|_| "redacted"))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_share_password_never_reaches_a_log_line() {
        let request = NewShare {
            path: "photos".to_owned(),
            expires_at: None,
            password: Some("correct horse battery".to_owned()),
        };
        let printed = format!("{request:?}");
        assert!(!printed.contains("correct horse battery"));
        assert!(printed.contains("redacted"));
    }
}
