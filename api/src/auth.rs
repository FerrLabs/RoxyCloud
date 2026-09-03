use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: Uuid,
    exp: i64,
}

pub struct Sessions {
    encoding: EncodingKey,
    decoding: DecodingKey,
    validation: Validation,
    ttl: Duration,
}

#[derive(Debug, thiserror::Error)]
#[error("signing the session token failed")]
pub struct SignFailed;

const CLOCK_SKEW_LEEWAY_SECONDS: u64 = 5;

impl Sessions {
    #[must_use]
    pub fn new(secret: &str, ttl: Duration) -> Self {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_required_spec_claims(&["sub", "exp"]);
        validation.leeway = CLOCK_SKEW_LEEWAY_SECONDS;
        Self {
            encoding: EncodingKey::from_secret(secret.as_bytes()),
            decoding: DecodingKey::from_secret(secret.as_bytes()),
            validation,
            ttl,
        }
    }

    pub fn issue(&self, user_id: Uuid) -> Result<String, SignFailed> {
        let claims = Claims {
            sub: user_id,
            exp: (Utc::now() + self.ttl).timestamp(),
        };
        encode(&Header::new(Algorithm::HS256), &claims, &self.encoding).map_err(|_| SignFailed)
    }

    #[must_use]
    pub fn verify(&self, token: &str) -> Option<Uuid> {
        decode::<Claims>(token, &self.decoding, &self.validation)
            .ok()
            .map(|data| data.claims.sub)
    }

    #[must_use]
    pub fn ttl_seconds(&self) -> i64 {
        self.ttl.num_seconds()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Caller {
    pub user_id: Uuid,
}

impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or(ApiError::Unauthenticated)?;

        state
            .sessions
            .verify(token.trim())
            .map(|user_id| Self { user_id })
            .ok_or(ApiError::Unauthenticated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sessions(ttl: Duration) -> Sessions {
        Sessions::new("test-secret", ttl)
    }

    #[test]
    fn a_freshly_issued_token_names_its_user() {
        let user_id = Uuid::now_v7();
        let sessions = sessions(Duration::hours(1));
        let token = sessions.issue(user_id).expect("signs");
        assert_eq!(sessions.verify(&token), Some(user_id));
    }

    #[test]
    fn an_expired_token_is_refused() {
        let sessions = sessions(Duration::hours(-1));
        let token = sessions.issue(Uuid::now_v7()).expect("signs");
        assert_eq!(sessions.verify(&token), None);
    }

    #[test]
    fn expiry_tolerance_stays_within_the_declared_clock_skew() {
        let user_id = Uuid::now_v7();
        let inside = sessions(Duration::seconds(-1));
        let token = inside.issue(user_id).expect("signs");
        assert_eq!(
            inside.verify(&token),
            Some(user_id),
            "clock skew is allowed"
        );

        let outside = sessions(Duration::seconds(
            -2 * i64::try_from(CLOCK_SKEW_LEEWAY_SECONDS).expect("small constant"),
        ));
        let token = outside.issue(user_id).expect("signs");
        assert_eq!(outside.verify(&token), None, "beyond skew must be refused");
    }

    #[test]
    fn a_token_signed_with_another_secret_is_refused() {
        let token = sessions(Duration::hours(1))
            .issue(Uuid::now_v7())
            .expect("signs");
        let attacker = Sessions::new("other-secret", Duration::hours(1));
        assert_eq!(attacker.verify(&token), None);
    }

    #[test]
    fn a_tampered_token_is_refused() {
        let sessions = sessions(Duration::hours(1));
        let token = sessions.issue(Uuid::now_v7()).expect("signs");
        let mut tampered = token.clone();
        tampered.pop();
        tampered.push('A');
        assert_eq!(sessions.verify(&tampered), None);
    }

    #[test]
    fn garbage_is_refused_without_panicking() {
        let sessions = sessions(Duration::hours(1));
        assert_eq!(sessions.verify(""), None);
        assert_eq!(sessions.verify("not.a.token"), None);
    }

    #[test]
    fn an_unsigned_alg_none_token_is_refused() {
        let forged = format!(
            "{}.{}.",
            base64_url(br#"{"alg":"none","typ":"JWT"}"#),
            base64_url(
                format!(
                    r#"{{"sub":"{}","exp":{}}}"#,
                    Uuid::now_v7(),
                    (Utc::now() + Duration::hours(1)).timestamp()
                )
                .as_bytes()
            )
        );
        assert_eq!(sessions(Duration::hours(1)).verify(&forged), None);
    }

    fn base64_url(raw: &[u8]) -> String {
        use std::fmt::Write as _;
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in raw.chunks(3) {
            let b = [
                chunk[0],
                chunk.get(1).copied().unwrap_or(0),
                chunk.get(2).copied().unwrap_or(0),
            ];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..=chunk.len() {
                let _ = write!(
                    out,
                    "{}",
                    ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char
                );
            }
        }
        out
    }
}
