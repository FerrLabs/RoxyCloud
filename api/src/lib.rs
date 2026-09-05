pub mod app_passwords;
pub mod auth;
pub mod config;
pub mod dav;
pub mod db;
pub mod error;
pub mod password;
pub mod routes;
pub mod state;
pub mod storage;
pub mod sweeper;
pub mod trash;
pub mod users;

use std::path::Path;

use axum::Router;
use axum::http::{HeaderValue, Method, header};
use axum::middleware::map_response;
use axum::response::Response;
use axum::routing::any;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::error::ApiError;
use crate::state::AppState;

pub fn build_router(
    state: AppState,
    allowed_origins: &[String],
    web_root: Option<&Path>,
) -> Router {
    let origins: Vec<HeaderValue> = allowed_origins
        .iter()
        .filter_map(|origin| origin.parse().ok())
        .collect();

    let cors = CorsLayer::new()
        .allow_origin(origins)
        .allow_credentials(true)
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE]);

    // CORS wraps the REST surface alone: the layer answers every OPTIONS itself, which would take
    // the one WebDAV clients read to decide what the server supports.
    let api = routes::router(state.clone()).layer(cors);
    let router = api.merge(dav::router().with_state(state));

    let router = match web_root {
        Some(root) => router
            .route("/v1", any(async || ApiError::NotFound))
            .route("/v1/", any(async || ApiError::NotFound))
            .route("/v1/{*unknown}", any(async || ApiError::NotFound))
            .fallback_service(ServeDir::new(root).fallback(ServeFile::new(root.join("index.html"))))
            .layer(map_response(revalidate_html)),
        None => router,
    };

    router.layer(TraceLayer::new_for_http())
}

async fn revalidate_html(mut response: Response) -> Response {
    let html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/html"));

    if html {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }
    response
}
