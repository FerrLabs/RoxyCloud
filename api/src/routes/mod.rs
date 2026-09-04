pub mod auth;
pub mod files;

use axum::Json;
use axum::routing::get;
use axum::{Router, routing::post, routing::put};
use serde_json::{Value, json};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/auth/login", post(auth::login))
        .route("/v1/auth/me", get(auth::me))
        .route("/v1/move", post(files::rename))
        .route("/v1/folders", get(files::list_root))
        .route("/v1/folders/{*path}", get(files::list))
        .route(
            "/v1/files/{*path}",
            put(files::put).get(files::get).delete(files::delete),
        )
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}
