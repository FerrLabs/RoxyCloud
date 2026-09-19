use serde::{Deserialize, Serialize};

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
