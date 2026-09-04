pub mod auth;
pub mod config;
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

    let router = match web_root {
        Some(root) => routes::router(state)
            .route("/v1/{*unknown}", any(async || ApiError::NotFound))
            .fallback_service(
                ServeDir::new(root).fallback(ServeFile::new(root.join("index.html"))),
            ),
        None => routes::router(state),
    };

    router.layer(cors).layer(TraceLayer::new_for_http())
}
