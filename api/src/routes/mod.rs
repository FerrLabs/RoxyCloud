pub mod app_passwords;
pub mod auth;
pub mod files;
pub mod oidc;
pub mod search;
pub mod shares;
pub mod thumbnails;
pub mod trash;
pub mod uploads;
pub mod users;

use axum::Json;
use axum::routing::get;
use axum::{Router, routing::delete, routing::post, routing::put};
use serde_json::{Value, json};

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/auth/login", post(auth::login))
        .route("/v1/auth/me", get(auth::me))
        .route(
            "/v1/auth/methods",
            get(oidc::methods).put(oidc::set_methods),
        )
        .route("/v1/auth/oidc/start", post(oidc::begin))
        .route("/v1/auth/oidc/callback", post(oidc::callback))
        .route(
            "/v1/app-passwords",
            get(app_passwords::list).post(app_passwords::mint),
        )
        .route("/v1/app-passwords/{id}", delete(app_passwords::revoke))
        .route("/v1/auth/password", put(users::change_password))
        .route("/v1/users", get(users::list).post(users::create))
        .route("/v1/users/{id}/disable", post(users::disable))
        .route("/v1/users/{id}/enable", post(users::enable))
        .route("/v1/users/{id}/unlock", post(users::unlock))
        .route("/v1/users/{id}/role", put(users::set_role))
        .route("/v1/users/{id}/quota", put(users::set_quota))
        .route("/v1/users/{id}/password", put(users::reset_password))
        .route("/v1/shares", get(shares::list).post(shares::create))
        .route("/v1/shares/{id}", delete(shares::revoke))
        .route("/v1/public/{token}", get(shares::open))
        .route("/v1/public/{token}/content", get(shares::download))
        .route("/v1/public/{token}/entries/{*path}", get(shares::open_at))
        .route(
            "/v1/public/{token}/content/{*path}",
            get(shares::download_at),
        )
        .route("/v1/search", get(search::search))
        .route("/v1/thumbnails/{*path}", get(thumbnails::get))
        .route("/v1/uploads", get(uploads::mine).post(uploads::begin))
        .route(
            "/v1/uploads/{id}",
            get(uploads::status)
                .patch(uploads::append)
                .delete(uploads::abandon),
        )
        .route("/v1/uploads/{id}/finish", post(uploads::finish))
        .route("/v1/move", post(files::rename))
        .route("/v1/trash", get(trash::list))
        .route("/v1/trash/{id}", delete(trash::purge))
        .route("/v1/trash/{id}/restore", post(trash::restore))
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
