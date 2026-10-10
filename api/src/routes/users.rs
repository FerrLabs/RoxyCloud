use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::attempts::{self, Scope};
use crate::auth::{Admin, SessionCaller};
use crate::error::ApiError;
use crate::state::AppState;
use crate::{password, sessions, users};
use stashden_core::role::Role;
use stashden_core::user::{Email, User};

#[derive(Deserialize)]
pub struct NewAccount {
    email: String,
    display_name: String,
    password: String,
    #[serde(default = "member")]
    role: Role,
}

const fn member() -> Role {
    Role::Member
}

#[derive(Deserialize)]
pub struct NewRole {
    role: Role,
}

#[derive(Deserialize)]
pub struct NewQuota {
    bytes_max: i64,
}

#[derive(Deserialize)]
pub struct NewPassword {
    password: String,
}

#[derive(Deserialize)]
pub struct PasswordChange {
    current: String,
    password: String,
}

#[derive(Serialize)]
pub struct Account {
    #[serde(flatten)]
    user: User,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes_used: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bytes_max: Option<i64>,
}

impl Account {
    fn of(user: User, quota: Option<(i64, i64)>) -> Self {
        Self {
            user,
            bytes_used: quota.map(|(used, _)| used),
            bytes_max: quota.map(|(_, max)| max),
        }
    }
}

pub async fn create(
    State(state): State<AppState>,
    _: Admin,
    Json(request): Json<NewAccount>,
) -> Result<(StatusCode, Json<User>), ApiError> {
    let email: Email = request.email.parse()?;

    let mut tx = state.db.begin().await?;
    let created = users::create(
        &mut tx,
        &email,
        request.display_name.trim(),
        &request.password,
        request.role,
    )
    .await?;
    tx.commit().await?;

    Ok((StatusCode::CREATED, Json(created)))
}

pub async fn list(State(state): State<AppState>, _: Admin) -> Result<Json<Vec<Account>>, ApiError> {
    let mut accounts = Vec::new();
    for user in users::list(&state.db).await? {
        let quota = users::usage(&state.db, user.id).await?;
        accounts.push(Account::of(user, quota));
    }
    Ok(Json(accounts))
}

pub async fn disable(
    State(state): State<AppState>,
    admin: Admin,
    Path(id): Path<Uuid>,
) -> Result<Json<User>, ApiError> {
    // An administrator who disables themselves leaves an installation nobody can administer.
    if admin.user.id == id {
        return Err(ApiError::WrongKind {
            expected: "account other than your own",
        });
    }
    let mut tx = state.db.begin().await?;
    let disabled = users::set_disabled(&mut *tx, id, true).await?;
    sessions::revoke_all(&mut *tx, id, None).await?;
    tx.commit().await?;
    Ok(Json(disabled))
}

#[derive(Deserialize)]
pub struct Departure {
    hand_over_to: Option<Uuid>,
}

pub async fn delete(
    State(state): State<AppState>,
    admin: Admin,
    Path(id): Path<Uuid>,
    Query(departure): Query<Departure>,
) -> Result<StatusCode, ApiError> {
    if admin.user.id == id {
        return Err(ApiError::WrongKind {
            expected: "account other than your own",
        });
    }
    if departure.hand_over_to == Some(id) {
        return Err(ApiError::WrongKind {
            expected: "account other than the one being deleted to hand the files to",
        });
    }
    crate::departure::remove(
        &state.db,
        id,
        departure.hand_over_to,
        state.default_quota_bytes,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn sign_out_everywhere(
    State(state): State<AppState>,
    _: Admin,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    users::by_id(&state.db, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    sessions::revoke_all(&state.db, id, None).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn enable(
    State(state): State<AppState>,
    _: Admin,
    Path(id): Path<Uuid>,
) -> Result<Json<User>, ApiError> {
    Ok(Json(users::set_disabled(&state.db, id, false).await?))
}

/// The counter that stops somebody guessing an account belongs to the account, not to whoever is
/// guessing, so an attacker can hold its owner out by failing against it. Waiting for them to lose
/// interest is not a recovery plan; this is the way back in.
pub async fn unlock(
    State(state): State<AppState>,
    _: Admin,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let user = users::by_id(&state.db, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    attempts::forget(&state.db, Scope::Login, user.email.as_str()).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn set_role(
    State(state): State<AppState>,
    admin: Admin,
    Path(id): Path<Uuid>,
    Json(request): Json<NewRole>,
) -> Result<Json<User>, ApiError> {
    if admin.user.id == id && !request.role.may_administer() {
        return Err(ApiError::WrongKind {
            expected: "account other than your own to demote",
        });
    }
    Ok(Json(users::set_role(&state.db, id, request.role).await?))
}

pub async fn set_quota(
    State(state): State<AppState>,
    _: Admin,
    Path(id): Path<Uuid>,
    Json(request): Json<NewQuota>,
) -> Result<StatusCode, ApiError> {
    users::set_quota(&state.db, id, request.bytes_max).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// An administrator resetting someone's password does not need to know the old one, which is the
/// point: the person who forgot it cannot supply it.
pub async fn reset_password(
    State(state): State<AppState>,
    _: Admin,
    Path(id): Path<Uuid>,
    Json(request): Json<NewPassword>,
) -> Result<StatusCode, ApiError> {
    let mut tx = state.db.begin().await?;
    users::set_password(&mut *tx, id, &request.password).await?;
    sessions::revoke_all(&mut *tx, id, None).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Changing your own password asks for the current one, so a borrowed session cannot lock the owner
/// out of their own account.
pub async fn change_password(
    State(state): State<AppState>,
    caller: SessionCaller,
    Json(request): Json<PasswordChange>,
) -> Result<StatusCode, ApiError> {
    attempts::spend(&state.db, Scope::Login, caller.user.email.as_str()).await?;
    if !password::verify(&request.current, &caller.user.password_hash) {
        return Err(ApiError::InvalidCredentials);
    }
    attempts::forget(&state.db, Scope::Login, caller.user.email.as_str()).await?;
    let mut tx = state.db.begin().await?;
    users::set_password(&mut *tx, caller.user.id, &request.password).await?;
    sessions::revoke_all(&mut *tx, caller.user.id, Some(caller.session_id)).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
