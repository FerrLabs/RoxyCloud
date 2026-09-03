use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::Type))]
#[cfg_attr(
    feature = "postgres",
    sqlx(type_name = "user_role", rename_all = "lowercase")
)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Member,
    Reader,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{0} is not a role")]
pub struct UnknownRole(String);

impl Role {
    #[must_use]
    pub const fn may_write(self) -> bool {
        matches!(self, Self::Admin | Self::Member)
    }

    #[must_use]
    pub const fn may_administer(self) -> bool {
        matches!(self, Self::Admin)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Member => "member",
            Self::Reader => "reader",
        }
    }
}

impl FromStr for Role {
    type Err = UnknownRole;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "admin" => Ok(Self::Admin),
            "member" => Ok(Self::Member),
            "reader" => Ok(Self::Reader),
            other => Err(UnknownRole(other.to_owned())),
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reader_cannot_write() {
        assert!(!Role::Reader.may_write());
        assert!(Role::Member.may_write());
        assert!(Role::Admin.may_write());
    }

    #[test]
    fn only_an_administrator_administers() {
        assert!(Role::Admin.may_administer());
        assert!(!Role::Member.may_administer());
        assert!(!Role::Reader.may_administer());
    }

    #[test]
    fn round_trips_through_its_wire_form() {
        for role in [Role::Admin, Role::Member, Role::Reader] {
            assert_eq!(role.as_str().parse(), Ok(role));
            assert_eq!(
                serde_json::from_str::<Role>(&format!("\"{role}\"")).expect("parses"),
                role
            );
            assert_eq!(
                serde_json::to_string(&role).expect("serialises"),
                format!("\"{role}\"")
            );
        }
    }

    #[test]
    fn an_unknown_role_is_refused_rather_than_defaulted() {
        assert_eq!(
            "superuser".parse::<Role>(),
            Err(UnknownRole("superuser".to_owned()))
        );
        assert!(serde_json::from_str::<Role>("\"superuser\"").is_err());
    }
}
