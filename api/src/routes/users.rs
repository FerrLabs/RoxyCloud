use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::{Admin, Caller};
use crate::error::ApiError;
use crate::state::AppState;
use crate::{password, users};
use roxycloud_core::role::Role;
use roxycloud_core::user::{Email, User};

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
    Ok(Json(users::set_disabled(&state.db, id, true).await?))
}

pub async fn enable(
    State(state): State<AppState>,
    _: Admin,
    Path(id): Path<Uuid>,
) -> Result<Json<User>, ApiError> {
    Ok(Json(users::set_disabled(&state.db, id, false).await?))
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
    users::set_password(&state.db, id, &request.password).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Changing your own password asks for the current one, so a borrowed session cannot lock the owner
/// out of their own account.
pub async fn change_password(
    State(state): State<AppState>,
    caller: Caller,
    Json(request): Json<PasswordChange>,
) -> Result<StatusCode, ApiError> {
    if !password::verify(&request.current, &caller.user.password_hash) {
        return Err(ApiError::InvalidCredentials);
    }
    users::set_password(&state.db, caller.user.id, &request.password).await?;
    Ok(StatusCode::NO_CONTENT)
}
