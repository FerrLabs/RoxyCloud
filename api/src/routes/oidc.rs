use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::auth::Admin;
use crate::error::ApiError;
use crate::routes::auth::Session;
use crate::state::AppState;
use crate::{oidc, settings};

/// Ties a flow to the browser that started it. Without it, whoever holds a live `code` and `state`
/// pair can finish the flow in somebody else's browser, and that somebody is then signed in as the
/// attacker with their uploads filed into the attacker's account.
const FLOW_COOKIE: &str = "roxycloud_oidc";

#[derive(Serialize)]
pub struct Beginning {
    authorize_url: String,
    state: String,
}

#[derive(Deserialize)]
pub struct Returned {
    code: String,
    state: String,
}

#[derive(Serialize)]
pub struct Methods {
    password: bool,
    oidc: bool,
}

#[derive(Deserialize)]
pub struct NewMethods {
    password: bool,
}

/// What a sign-in screen needs to know before it draws itself, and the one route here that answers
/// without a session, because nobody has one yet.
pub async fn methods(State(state): State<AppState>) -> Result<Json<Methods>, ApiError> {
    Ok(Json(Methods {
        password: settings::password_login_allowed(&state.db).await?,
        oidc: state.oidc.is_some(),
    }))
}

pub async fn begin(State(state): State<AppState>) -> Result<Response, ApiError> {
    let provider = state.oidc.as_ref().ok_or(ApiError::NotFound)?;
    let found = oidc::discover(&state.http, &provider.issuer).await?;

    let started = oidc::start();
    oidc::remember(&state.db, &started).await?;

    let authorize_url = format!(
        "{}?response_type=code&scope=openid%20email%20profile&client_id={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method=S256",
        found.authorization_endpoint,
        urlencoding(&provider.client_id),
        urlencoding(&provider.redirect_url),
        urlencoding(&started.state),
        urlencoding(&started.challenge),
    );

    let cookie = format!(
        "{FLOW_COOKIE}={}; HttpOnly; SameSite=Lax; Path=/; Max-Age=900{}",
        started.state,
        if provider.redirect_url.starts_with("https://") {
            "; Secure"
        } else {
            ""
        }
    );

    Ok((
        [(
            header::SET_COOKIE,
            HeaderValue::from_str(&cookie).map_err(|_| ApiError::Credential)?,
        )],
        Json(Beginning {
            authorize_url,
            state: started.state,
        }),
    )
        .into_response())
}

/// The browser hands back the code it was given, and the state it was given with it. The verifier
/// never left this server, so a code intercepted on the way is not enough to finish the flow.
pub async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(returned): Json<Returned>,
) -> Result<Json<Session>, ApiError> {
    let provider = state.oidc.as_ref().ok_or(ApiError::NotFound)?;

    // The state the browser was given when it started, which an attacker who merely holds a code
    // cannot produce in somebody else's browser.
    if cookie(&headers, FLOW_COOKIE).is_none_or(|carried| carried != returned.state) {
        return Err(ApiError::Unauthenticated);
    }

    let verifier = oidc::claim_flow(&state.db, &returned.state).await?;
    let found = oidc::discover(&state.http, &provider.issuer).await?;

    let claims = oidc::claims_for(
        &state.http,
        &found,
        &provider.client_id,
        &provider.client_secret,
        &provider.redirect_url,
        &returned.code,
        &verifier,
    )
    .await?;

    let user = oidc::admit(&state.db, &claims, provider.create_accounts)
        .await
        .map_err(|refused| match refused {
            oidc::RefusedOrFailed::Refused(_) => ApiError::InvalidCredentials,
            oidc::RefusedOrFailed::Failed(failure) => failure,
        })?;

    if !user.is_active() {
        return Err(ApiError::Unauthenticated);
    }

    Ok(Json(Session {
        token: state.sessions.issue(user.id)?,
        expires_in: state.sessions.ttl_seconds(),
        user,
    }))
}

/// Turning password login off is what makes a provider the only way in, and it is deliberately not
/// a config flag: an administrator does it once the provider is known to work, from the running
/// system, rather than by restarting it with a different environment and hoping.
pub async fn set_methods(
    State(state): State<AppState>,
    _: Admin,
    Json(request): Json<NewMethods>,
) -> Result<Json<Methods>, ApiError> {
    if !request.password && state.oidc.is_none() {
        return Err(ApiError::WrongKind {
            expected: "provider to sign in with before passwords are turned off",
        });
    }

    settings::allow_password_login(&state.db, request.password).await?;
    Ok(Json(Methods {
        password: request.password,
        oidc: state.oidc.is_some(),
    }))
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
}

fn urlencoding(raw: &str) -> String {
    percent_encoding::utf8_percent_encode(raw, percent_encoding::NON_ALPHANUMERIC).to_string()
}
