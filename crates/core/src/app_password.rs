use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct AppPassword {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Minted {
    #[serde(flatten)]
    pub password: AppPassword,
    pub secret: String,
}

impl std::fmt::Debug for Minted {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Minted")
            .field("password", &self.password)
            .field("secret", &"redacted")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minted_secret_never_reaches_a_log_line() {
        let minted = Minted {
            password: AppPassword {
                id: Uuid::nil(),
                name: "laptop".to_owned(),
                created_at: Utc::now(),
                last_used_at: None,
            },
            secret: "0123456789abcdef".to_owned(),
        };
        let printed = format!("{minted:?}");
        assert!(!printed.contains("0123456789abcdef"));
        assert!(printed.contains("laptop"));
    }

    #[test]
    fn the_wire_shape_keeps_the_password_fields_beside_the_secret() {
        let wire = serde_json::json!({
            "id": Uuid::nil(),
            "name": "laptop",
            "created_at": "2026-10-06T08:00:00Z",
            "last_used_at": null,
            "secret": "0123456789abcdef",
        });
        let minted: Minted = serde_json::from_value(wire.clone()).expect("the API's answer parses");
        assert_eq!(minted.password.name, "laptop");
        assert_eq!(serde_json::to_value(&minted).expect("serializes"), wire);
    }
}
