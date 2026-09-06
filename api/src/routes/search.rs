use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;

use crate::auth::Caller;
use crate::error::ApiError;
use crate::search::{self, DEFAULT_LIMIT, Hit};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct Search {
    q: String,
    limit: Option<i64>,
    offset: Option<i64>,
}

pub async fn search(
    State(state): State<AppState>,
    caller: Caller,
    Query(request): Query<Search>,
) -> Result<Json<Vec<Hit>>, ApiError> {
    Ok(Json(
        search::by_name(
            &state.db,
            caller.user_id(),
            &request.q,
            request.limit.unwrap_or(DEFAULT_LIMIT),
            request.offset.unwrap_or(0),
        )
        .await?,
    ))
}
