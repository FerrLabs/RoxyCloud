use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::node::NodeKind;

pub const SHARED_WITH_ME: &str = "Shared with me";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::Type))]
#[cfg_attr(
    feature = "postgres",
    sqlx(type_name = "grant_access", rename_all = "lowercase")
)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    Read,
    Write,
}

impl Access {
    #[must_use]
    pub const fn may_write(self) -> bool {
        matches!(self, Self::Write)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct Given {
    pub id: Uuid,
    pub node_id: Uuid,
    pub name: String,
    pub kind: NodeKind,
    pub email: String,
    pub access: Access,
    pub in_trash: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct Received {
    pub id: Uuid,
    pub name: String,
    pub kind: NodeKind,
    pub access: Access,
    pub owner_email: String,
    pub owner_name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewGrant {
    pub path: String,
    pub email: String,
    pub access: Access,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_write_access_writes() {
        assert!(Access::Write.may_write());
        assert!(!Access::Read.may_write());
    }

    #[test]
    fn access_travels_as_a_lowercase_word() {
        assert_eq!(
            serde_json::to_string(&Access::Write).expect("serialisable"),
            "\"write\""
        );
        assert_eq!(
            serde_json::from_str::<Access>("\"read\"").expect("a known access"),
            Access::Read
        );
    }
}
