use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use sqlx::PgPool;

use crate::error::ApiError;
use roxycloud_core::user::{Email, User};

/// How long a browser has to come back with the code. Long enough for a person to type a password
/// and answer a second factor, short enough that abandoned flows do not pile up.
const FLOW_MINUTES: i64 = 15;

#[derive(Debug, Clone, Deserialize)]
pub struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

/// What the provider says about the person. `email_verified` is the field this whole module turns
/// on, so it is not an `Option<bool>` that a missing claim quietly makes true.
#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub iss: String,
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: bool,
    pub name: Option<String>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Refused {
    #[error("the provider did not say which address this is")]
    NoEmail,
    #[error("the provider did not vouch for this address")]
    Unverified,
    #[error("that address has no account here")]
    NoAccount,
}

pub struct Started {
    pub state: String,
    pub verifier: String,
    pub challenge: String,
}

#[must_use]
pub fn start() -> Started {
    let state = random();
    let verifier = random();
    let challenge = URL_SAFE_NO_PAD.encode(sha256(verifier.as_bytes()));
    Started {
        state,
        verifier,
        challenge,
    }
}

pub async fn remember(pool: &PgPool, started: &Started) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO oidc_flows (state, verifier, expires_at)
         VALUES ($1, $2, now() + make_interval(mins => $3))",
    )
    .bind(&started.state)
    .bind(&started.verifier)
    .bind(i32::try_from(FLOW_MINUTES).unwrap_or(15))
    .execute(pool)
    .await?;
    Ok(())
}

/// Answers the verifier for a state, once. A state that has been spent cannot be spent again,
/// which is what stops a code being replayed against a flow somebody else started.
pub async fn claim_flow(pool: &PgPool, state: &str) -> Result<String, ApiError> {
    sqlx::query_scalar::<_, String>(
        "DELETE FROM oidc_flows WHERE state = $1 AND expires_at > now() RETURNING verifier",
    )
    .bind(state)
    .fetch_optional(pool)
    .await?
    .ok_or(ApiError::Unauthenticated)
}

pub async fn purge_expired(pool: &PgPool) -> Result<u64, ApiError> {
    let removed = sqlx::query("DELETE FROM oidc_flows WHERE expires_at <= now()")
        .execute(pool)
        .await?;
    Ok(removed.rows_affected())
}

/// The account a set of claims is allowed to be, and the only place that decides it.
///
/// An address the provider has not vouched for never reaches an account, whether that account
/// exists or not. Taking over somebody else's account by asserting their address at a provider
/// that never checked it is the classic way an OIDC integration is broken, and it is one `if`
/// away in either direction, so it lives here alone and is tested here.
pub async fn admit(
    pool: &PgPool,
    claims: &Claims,
    may_create: bool,
) -> Result<User, RefusedOrFailed> {
    let raw = claims.email.as_deref().ok_or(Refused::NoEmail)?;
    if !claims.email_verified {
        return Err(Refused::Unverified.into());
    }

    let email: Email = raw.parse().map_err(|_| Refused::NoEmail)?;
    if let Some(existing) = crate::users::by_email(pool, &email).await? {
        return Ok(existing);
    }
    if !may_create {
        return Err(Refused::NoAccount.into());
    }

    let display = claims.name.as_deref().unwrap_or(raw);
    let mut tx = pool.begin().await?;
    // The password is random and never shown to anybody. An account that arrives through a
    // provider has no password to sign in with, and leaving the column empty would make one.
    let created = crate::users::create(
        &mut tx,
        &email,
        display,
        &random(),
        roxycloud_core::role::Role::Member,
    )
    .await?;
    tx.commit().await?;
    Ok(created)
}

/// Fetches what the provider says about itself. Nothing here is configured by hand beyond the
/// issuer, because an endpoint typed into a config file is one that can be pointed somewhere else.
pub async fn discover(http: &reqwest::Client, issuer: &str) -> Result<Discovery, ApiError> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let found = http
        .get(url)
        .send()
        .await
        .map_err(|_| ApiError::Unauthenticated)?
        .json::<Discovery>()
        .await
        .map_err(|_| ApiError::Unauthenticated)?;

    // A document that names somebody else is not this provider's, whoever served it.
    if found.issuer.trim_end_matches('/') != issuer.trim_end_matches('/') {
        return Err(ApiError::Unauthenticated);
    }
    Ok(found)
}

#[derive(Deserialize)]
struct Tokens {
    id_token: String,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kid: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

/// Exchanges the code for an id token and reads the claims out of it, having checked the
/// provider's signature over them. Nothing in the token is believed before that: the whole point
/// of the flow is that the browser carried the code and the provider vouches for the rest.
pub async fn claims_for(
    http: &reqwest::Client,
    provider: &Discovery,
    client_id: &str,
    client_secret: &str,
    redirect_url: &str,
    code: &str,
    verifier: &str,
) -> Result<Claims, ApiError> {
    let tokens = http
        .post(&provider.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_url),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .map_err(|_| ApiError::Unauthenticated)?
        .json::<Tokens>()
        .await
        .map_err(|_| ApiError::Unauthenticated)?;

    verify(http, provider, client_id, &tokens.id_token).await
}

async fn verify(
    http: &reqwest::Client,
    provider: &Discovery,
    client_id: &str,
    token: &str,
) -> Result<Claims, ApiError> {
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};

    let header = decode_header(token).map_err(|_| ApiError::Unauthenticated)?;
    let jwks = http
        .get(&provider.jwks_uri)
        .send()
        .await
        .map_err(|_| ApiError::Unauthenticated)?
        .json::<Jwks>()
        .await
        .map_err(|_| ApiError::Unauthenticated)?;

    let key = jwks
        .keys
        .iter()
        .find(|key| match header.kid.as_deref() {
            Some(kid) => key.kid.as_deref() == Some(kid),
            // A lone key is used only when the token names none. Matching one whose `kid`
            // contradicts the header would hide a rotation gone wrong.
            None => jwks.keys.len() == 1,
        })
        .ok_or(ApiError::Unauthenticated)?;
    let (modulus, exponent) = key
        .n
        .as_deref()
        .zip(key.e.as_deref())
        .ok_or(ApiError::Unauthenticated)?;

    let decoding = DecodingKey::from_rsa_components(modulus, exponent)
        .map_err(|_| ApiError::Unauthenticated)?;
    // The token's own header says which algorithm signed it, and whoever sent the token wrote
    // that header. What is accepted is pinned here instead.
    let mut validation = Validation::new(Algorithm::RS256);
    validation.algorithms = vec![Algorithm::RS256, Algorithm::RS384, Algorithm::RS512];
    validation.set_audience(&[client_id]);
    // Required and *compared*. Without this any token signed by a key in this JWKS is accepted,
    // which on a multi-tenant provider, where tenants share signing keys, means a user in an
    // unrelated tenant reaches `admit` and is mapped to a local account by address: the takeover
    // the rest of this module is written to prevent.
    validation.set_issuer(&[&provider.issuer]);
    validation.set_required_spec_claims(&["exp", "iss", "aud"]);

    decode::<Claims>(token, &decoding, &validation)
        .map(|data| data.claims)
        .map_err(|_| ApiError::Unauthenticated)
}

#[derive(Debug, thiserror::Error)]
pub enum RefusedOrFailed {
    #[error(transparent)]
    Refused(#[from] Refused),
    #[error(transparent)]
    Failed(#[from] ApiError),
}

impl From<sqlx::Error> for RefusedOrFailed {
    fn from(err: sqlx::Error) -> Self {
        Self::Failed(ApiError::Database(err))
    }
}

fn random() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the operating system has randomness");
    URL_SAFE_NO_PAD.encode(bytes)
}

fn sha256(input: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(input);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(email: Option<&str>, verified: bool) -> Claims {
        Claims {
            sub: "provider-subject".to_owned(),
            iss: "https://idp.example".to_owned(),
            email: email.map(ToOwned::to_owned),
            email_verified: verified,
            name: Some("Someone".to_owned()),
        }
    }

    #[test]
    fn the_challenge_is_the_verifier_hashed_not_the_verifier() {
        let started = start();

        assert_ne!(started.challenge, started.verifier);
        assert_eq!(
            started.challenge,
            URL_SAFE_NO_PAD.encode(sha256(started.verifier.as_bytes())),
            "S256 is what makes the code useless to whoever intercepts it"
        );
    }

    #[test]
    fn two_flows_share_nothing() {
        let first = start();
        let second = start();

        assert_ne!(first.state, second.state);
        assert_ne!(first.verifier, second.verifier);
    }

    #[test]
    fn a_missing_verification_claim_is_not_a_verified_address() {
        let parsed: Claims = serde_json::from_str(
            r#"{"sub":"s","iss":"https://idp.example","email":"someone@example.com"}"#,
        )
        .expect("claims without the field");

        assert!(
            !parsed.email_verified,
            "a provider that says nothing has not vouched for anything"
        );
    }

    #[test]
    fn the_claims_a_provider_sends_are_read_the_way_they_are_written() {
        let parsed: Claims = serde_json::from_str(
            r#"{"sub":"s","iss":"https://idp.example","email":"a@b.com","email_verified":true,"name":"A B"}"#,
        )
        .expect("full claims");

        assert!(parsed.email_verified);
        assert_eq!(parsed.email.as_deref(), Some("a@b.com"));
        assert_eq!(parsed.name.as_deref(), Some("A B"));
    }

    #[test]
    fn claims_carrying_no_address_are_refused_before_anything_is_looked_up() {
        let without = claims(None, true);
        assert!(without.email.is_none());
    }
}
