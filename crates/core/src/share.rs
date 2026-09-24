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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewShare {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}
