use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::app_passwords;
use crate::dav::auth::basic_credentials;
use crate::error::ApiError;
use crate::sessions;
use crate::state::AppState;
use stashden_core::user::User;

#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    sub: Uuid,
    sid: Uuid,
    exp: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verified {
    pub user_id: Uuid,
    pub session_id: Uuid,
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

    pub async fn open(&self, pool: &PgPool, user_id: Uuid) -> Result<String, ApiError> {
        let session_id = sessions::create(pool, user_id, Utc::now() + self.ttl).await?;
        Ok(self.issue(user_id, session_id)?)
    }

    fn issue(&self, user_id: Uuid, session_id: Uuid) -> Result<String, SignFailed> {
        let claims = Claims {
            sub: user_id,
            sid: session_id,
            exp: (Utc::now() + self.ttl).timestamp(),
        };
        encode(&Header::new(Algorithm::HS256), &claims, &self.encoding).map_err(|_| SignFailed)
    }

    #[must_use]
    pub fn verify(&self, token: &str) -> Option<Verified> {
        decode::<Claims>(token, &self.decoding, &self.validation)
            .ok()
            .map(|data| Verified {
                user_id: data.claims.sub,
                session_id: data.claims.sid,
            })
    }

    #[must_use]
    pub fn ttl_seconds(&self) -> i64 {
        self.ttl.num_seconds()
    }
}

/// The account behind a session token, loaded rather than taken on the token's word: a session
/// outliving the account it names is how a disabled person keeps reading until their token expires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Credential {
    Session(Uuid),
    AppPassword(Uuid),
}

#[derive(Debug, Clone)]
pub struct Caller {
    pub user: User,
    pub via: Credential,
}

impl Caller {
    #[must_use]
    pub fn user_id(&self) -> Uuid {
        self.user.id
    }
}

#[derive(Debug, Clone)]
pub struct Writer {
    pub user: User,
}

impl Writer {
    #[must_use]
    pub fn user_id(&self) -> Uuid {
        self.user.id
    }
}

impl FromRequestParts<AppState> for Writer {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let caller = Caller::from_request_parts(parts, state).await?;
        if !caller.user.may_write() {
            return Err(ApiError::Forbidden);
        }
        Ok(Self { user: caller.user })
    }
}

/// An administrator, for the routes that change other people's accounts.
#[derive(Debug, Clone)]
pub struct Admin {
    pub user: User,
}

impl FromRequestParts<AppState> for Admin {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let SessionCaller { user, .. } = SessionCaller::from_request_parts(parts, state).await?;
        if !user.may_administer() {
            return Err(ApiError::Forbidden);
        }
        Ok(Self { user })
    }
}

#[derive(Debug, Clone)]
pub struct SessionCaller {
    pub user: User,
    pub session_id: Uuid,
}

impl SessionCaller {
    #[must_use]
    pub fn user_id(&self) -> Uuid {
        self.user.id
    }
}

impl FromRequestParts<AppState> for SessionCaller {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let caller = Caller::from_request_parts(parts, state).await?;
        let Credential::Session(session_id) = caller.via else {
            return Err(ApiError::SessionRequired);
        };
        Ok(Self {
            user: caller.user,
            session_id,
        })
    }
}

impl FromRequestParts<AppState> for Caller {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let bearer = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));

        let (user, via) = if let Some(token) = bearer {
            let verified = state
                .sessions
                .verify(token.trim())
                .ok_or(ApiError::Unauthenticated)?;
            if !sessions::is_live(&state.db, verified.session_id, verified.user_id).await? {
                return Err(ApiError::Unauthenticated);
            }
            let user = crate::users::by_id(&state.db, verified.user_id)
                .await?
                .ok_or(ApiError::Unauthenticated)?;
            (user, Credential::Session(verified.session_id))
        } else {
            let (email, secret) = basic_credentials(parts).ok_or(ApiError::Unauthenticated)?;
            let (user, id) = app_passwords::authenticate(&state.db, &email, &secret)
                .await
                .ok_or(ApiError::Unauthenticated)?;
            (user, Credential::AppPassword(id))
        };

        if !user.is_active() {
            return Err(ApiError::Unauthenticated);
        }
        Ok(Self { user, via })
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
        let token = sessions.issue(user_id, Uuid::now_v7()).expect("signs");
        assert_eq!(
            sessions.verify(&token).map(|verified| verified.user_id),
            Some(user_id)
        );
    }

    #[test]
    fn an_expired_token_is_refused() {
        let sessions = sessions(Duration::hours(-1));
        let token = sessions
            .issue(Uuid::now_v7(), Uuid::now_v7())
            .expect("signs");
        assert_eq!(sessions.verify(&token), None);
    }

    #[test]
    fn expiry_tolerance_stays_within_the_declared_clock_skew() {
        let user_id = Uuid::now_v7();
        let inside = sessions(Duration::seconds(-1));
        let token = inside.issue(user_id, Uuid::now_v7()).expect("signs");
        assert_eq!(
            inside.verify(&token).map(|verified| verified.user_id),
            Some(user_id),
            "clock skew is allowed"
        );

        let outside = sessions(Duration::seconds(
            -2 * i64::try_from(CLOCK_SKEW_LEEWAY_SECONDS).expect("small constant"),
        ));
        let token = outside.issue(user_id, Uuid::now_v7()).expect("signs");
        assert_eq!(outside.verify(&token), None, "beyond skew must be refused");
    }

    #[test]
    fn a_token_signed_with_another_secret_is_refused() {
        let token = sessions(Duration::hours(1))
            .issue(Uuid::now_v7(), Uuid::now_v7())
            .expect("signs");
        let attacker = Sessions::new("other-secret", Duration::hours(1));
        assert_eq!(attacker.verify(&token), None);
    }

    #[test]
    fn a_tampered_token_is_refused() {
        let sessions = sessions(Duration::hours(1));
        let token = sessions
            .issue(Uuid::now_v7(), Uuid::now_v7())
            .expect("signs");
        let mut tampered = token.clone();
        let last = tampered.pop().expect("a signed token is never empty");
        tampered.push(if last == 'A' { 'B' } else { 'A' });
        assert_ne!(
            tampered, token,
            "the tampering must actually change the token"
        );
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
                    r#"{{"sub":"{}","sid":"{}","exp":{}}}"#,
                    Uuid::now_v7(),
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
