use axum::extract::{FromRequestParts, State};
use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::app_passwords;
use crate::state::AppState;
use roxycloud_core::user::User;

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
                HeaderValue::from_static("Basic realm=\"RoxyCloud\", charset=\"UTF-8\""),
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
        let presented = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Basic "))
            .and_then(decode)
            .ok_or(Unauthenticated)?;

        let (email, secret) = presented.split_once(':').ok_or(Unauthenticated)?;
        let email = email.parse().map_err(|_| Unauthenticated)?;

        let State(state) = State::<AppState>::from_request_parts(parts, state)
            .await
            .map_err(|_| Unauthenticated)?;

        app_passwords::authenticate(&state.db, &email, secret)
            .await
            .map(Self)
            .ok_or(Unauthenticated)
    }
}

fn decode(encoded: &str) -> Option<String> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut bits = 0u32;
    let mut held = 0;
    let mut out = Vec::new();

    for byte in encoded.trim().bytes() {
        if byte == b'=' {
            break;
        }
        let value = ALPHABET.iter().position(|candidate| *candidate == byte)?;
        bits = (bits << 6) | u32::try_from(value).ok()?;
        held += 6;
        if held >= 8 {
            held -= 8;
            out.push(u8::try_from((bits >> held) & 0xff).ok()?);
        }
    }

    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::decode;

    #[test]
    fn a_credential_survives_the_round_trip() {
        assert_eq!(
            decode("ZGF2QGV4YW1wbGUuY29tOnNlY3JldA=="),
            Some("dav@example.com:secret".to_owned())
        );
    }

    #[test]
    fn padding_is_optional_and_whitespace_is_forgiven() {
        assert_eq!(decode("YTpi"), Some("a:b".to_owned()));
        assert_eq!(decode(" YTpi "), Some("a:b".to_owned()));
    }

    #[test]
    fn anything_outside_the_alphabet_is_refused() {
        assert_eq!(decode("not base64!"), None);
    }

    #[test]
    fn bytes_that_are_not_text_are_refused() {
        assert_eq!(decode("//8="), None);
    }
}
