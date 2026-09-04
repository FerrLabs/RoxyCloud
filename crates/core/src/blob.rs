use std::fmt;
use std::str::FromStr;

pub const HASH_LEN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlobHash([u8; HASH_LEN]);

#[derive(Debug, thiserror::Error)]
pub enum ParseHashError {
    #[error("blob hash is not valid hex")]
    NotHex(#[from] hex::FromHexError),
    #[error("blob hash must be {HASH_LEN} bytes, got {0}")]
    WrongLength(usize),
}

impl BlobHash {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; HASH_LEN]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; HASH_LEN] {
        &self.0
    }

    #[must_use]
    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }
}

impl TryFrom<&[u8]> for BlobHash {
    type Error = ParseHashError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        <[u8; HASH_LEN]>::try_from(bytes)
            .map(Self)
            .map_err(|_| ParseHashError::WrongLength(bytes.len()))
    }
}

impl FromStr for BlobHash {
    type Err = ParseHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut bytes = [0u8; HASH_LEN];
        hex::decode_to_slice(s, &mut bytes).map_err(|err| match err {
            hex::FromHexError::InvalidStringLength => ParseHashError::WrongLength(s.len() / 2),
            other => ParseHashError::NotHex(other),
        })?;
        Ok(Self(bytes))
    }
}

impl fmt::Display for BlobHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl From<blake3::Hash> for BlobHash {
    fn from(hash: blake3::Hash) -> Self {
        Self(*hash.as_bytes())
    }
}

#[cfg(feature = "postgres")]
mod postgres_impls {
    use super::BlobHash;
    use sqlx::{Postgres, encode::IsNull, error::BoxDynError, postgres::PgTypeInfo};

    impl sqlx::Type<Postgres> for BlobHash {
        fn type_info() -> PgTypeInfo {
            <[u8] as sqlx::Type<Postgres>>::type_info()
        }

        fn compatible(ty: &PgTypeInfo) -> bool {
            <[u8] as sqlx::Type<Postgres>>::compatible(ty)
        }
    }

    impl sqlx::Encode<'_, Postgres> for BlobHash {
        fn encode_by_ref(
            &self,
            buf: &mut <Postgres as sqlx::Database>::ArgumentBuffer,
        ) -> Result<IsNull, BoxDynError> {
            <&[u8] as sqlx::Encode<Postgres>>::encode(&self.0[..], buf)
        }
    }

    impl sqlx::Decode<'_, Postgres> for BlobHash {
        fn decode(value: <Postgres as sqlx::Database>::ValueRef<'_>) -> Result<Self, BoxDynError> {
            let bytes = <&[u8] as sqlx::Decode<Postgres>>::decode(value)?;
            Ok(Self::try_from(bytes)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        let hash = BlobHash::from(blake3::hash(b"roxycloud"));
        let parsed: BlobHash = hash.to_hex().parse().expect("hex parses back");
        assert_eq!(hash, parsed);
    }

    #[test]
    fn rejects_wrong_length() {
        let short = "ab".repeat(HASH_LEN - 1);
        assert!(matches!(
            short.parse::<BlobHash>(),
            Err(ParseHashError::WrongLength(_))
        ));
    }

    #[test]
    fn rejects_non_hex() {
        let bad = "z".repeat(HASH_LEN * 2);
        assert!(matches!(
            bad.parse::<BlobHash>(),
            Err(ParseHashError::NotHex(_))
        ));
    }

    #[test]
    fn rejects_short_byte_slice() {
        assert!(matches!(
            BlobHash::try_from(&[0u8; 8][..]),
            Err(ParseHashError::WrongLength(8))
        ));
    }
}
