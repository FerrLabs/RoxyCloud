use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_EMAIL_LEN: usize = 254;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Email(String);

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InvalidEmail {
    #[error("email is empty")]
    Empty,
    #[error("email is longer than {MAX_EMAIL_LEN} bytes")]
    TooLong,
    #[error("email needs a local part and a domain separated by @")]
    Shape,
}

impl Email {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Email {
    type Err = InvalidEmail;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(InvalidEmail::Empty);
        }
        if trimmed.len() > MAX_EMAIL_LEN {
            return Err(InvalidEmail::TooLong);
        }
        let (local, domain) = trimmed.split_once('@').ok_or(InvalidEmail::Shape)?;
        if local.is_empty() || domain.is_empty() || domain.contains('@') || !domain.contains('.') {
            return Err(InvalidEmail::Shape);
        }
        Ok(Self(trimmed.to_lowercase()))
    }
}

impl TryFrom<String> for Email {
    type Error = InvalidEmail;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<Email> for String {
    fn from(email: Email) -> Self {
        email.0
    }
}

impl fmt::Display for Email {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub display_name: String,
    pub is_admin: bool,
    #[serde(skip)]
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_at: Option<DateTime<Utc>>,
}

impl User {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.disabled_at.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_normalised_so_one_person_gets_one_account() {
        let a: Email = "  Bryan@Example.COM ".parse().expect("valid");
        let b: Email = "bryan@example.com".parse().expect("valid");
        assert_eq!(a, b);
        assert_eq!(a.as_str(), "bryan@example.com");
    }

    #[test]
    fn rejects_addresses_without_a_domain() {
        assert_eq!("bryan".parse::<Email>(), Err(InvalidEmail::Shape));
        assert_eq!("bryan@".parse::<Email>(), Err(InvalidEmail::Shape));
        assert_eq!("@example.com".parse::<Email>(), Err(InvalidEmail::Shape));
        assert_eq!("bryan@localhost".parse::<Email>(), Err(InvalidEmail::Shape));
    }

    #[test]
    fn rejects_a_second_at_sign() {
        assert_eq!("a@b@example.com".parse::<Email>(), Err(InvalidEmail::Shape));
    }

    #[test]
    fn rejects_blank_and_overlong_addresses() {
        assert_eq!("   ".parse::<Email>(), Err(InvalidEmail::Empty));
        let long = format!("{}@example.com", "x".repeat(MAX_EMAIL_LEN));
        assert_eq!(long.parse::<Email>(), Err(InvalidEmail::TooLong));
    }

    #[test]
    fn the_password_hash_never_leaves_through_serde() {
        let user = User {
            id: Uuid::now_v7(),
            email: "bryan@example.com".to_owned(),
            display_name: "Bryan".to_owned(),
            is_admin: true,
            password_hash: "$argon2id$v=19$m=19456,t=2,p=1$secret".to_owned(),
            created_at: Utc::now(),
            disabled_at: None,
        };
        let json = serde_json::to_string(&user).expect("serialises");
        assert!(!json.contains("argon2"), "{json}");
        assert!(!json.contains("password_hash"), "{json}");
    }
}
