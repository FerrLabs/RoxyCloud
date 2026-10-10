use axum::extract::{FromRequestParts, State};
use axum::http::header::WWW_AUTHENTICATE;
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::app_passwords;
use crate::auth::basic_credentials;
use crate::state::AppState;
use stashden_core::user::User;

/// A client that presented an app password over Basic auth. Session tokens are deliberately not
/// accepted here: this surface exists for credentials a client may keep on disk.
pub struct DavCaller(pub User);

pub struct Unauthenticated;

impl IntoResponse for Unauthenticated {
    fn into_response(self) -> Response {
        (
            StatusCode::UNAUTHORIZED,
            [(
                WWW_AUTHENTICATE,
                HeaderValue::from_static("Basic realm=\"Stashden\", charset=\"UTF-8\""),
            )],
        )
            .into_response()
    }
}

impl FromRequestParts<AppState> for DavCaller {
    type Rejection = Unauthenticated;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let (email, secret) = basic_credentials(parts).ok_or(Unauthenticated)?;

        let State(state) = State::<AppState>::from_request_parts(parts, state)
            .await
            .map_err(|_| Unauthenticated)?;

        app_passwords::authenticate(&state.db, &email, &secret)
            .await
            .map(|(user, _)| Self(user))
            .ok_or(Unauthenticated)
    }
}
