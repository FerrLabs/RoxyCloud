pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod password;
pub mod routes;
pub mod state;
pub mod storage;
pub mod users;

use axum::Router;
use axum::http::{HeaderValue, Method, header};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::state::AppState;

pub fn build_router(state: AppState, allowed_origins: &[String]) -> Router {
    let origins: Vec<HeaderValue> = allowed_origins
        .iter()
        .filter_map(|origin| origin.parse().ok())
        .collect();

    let cors = CorsLayer::new()
        .allow_origin(origins)
        .allow_credentials(true)
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE]);

    routes::router(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http())
}
