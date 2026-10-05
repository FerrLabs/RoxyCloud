mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use stashden_api::app_passwords::{mint, revoke};
use stashden_api::build_router;
use stashden_core::role::Role;
use tower::ServiceExt;
use uuid::Uuid;

use common::Harness;

struct Minted {
    owner: Uuid,
    id: Uuid,
    basic: String,
}

async fn app_password(harness: &Harness, email: &str, role: Role) -> Minted {
    let user = harness.account(email, role).await;
    let mut tx = harness.state.db.begin().await.expect("begin");
    let minted = mint(&mut tx, user.id, "stashden sync on the server")
        .await
        .expect("minting");
    tx.commit().await.expect("commit");
    Minted {
        owner: user.id,
        id: minted.password.id,
        basic: basic(email, &minted.secret),
    }
}

fn basic(email: &str, secret: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{email}:{secret}")))
}

async fn status(
    harness: &Harness,
    method: &str,
    path: &str,
    authorization: &str,
    body: &str,
) -> StatusCode {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, authorization)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_owned()))
        .expect("a well formed request");
    build_router(harness.state.clone(), &[], None)
        .oneshot(request)
        .await
        .expect("the router answers")
        .status()
}

database_test!(an_app_password_reaches_files_and_folders, harness, {
    let minted = app_password(&harness, "sync@example.com", Role::Member).await;
    harness
        .write(minted.owner, "notes/plan.md", b"the plan")
        .await;

    assert_eq!(
        status(&harness, "GET", "/v1/folders/notes", &minted.basic, "").await,
        StatusCode::OK
    );
    assert_eq!(
        status(
            &harness,
            "GET",
            "/v1/files/notes/plan.md",
            &minted.basic,
            ""
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        status(
            &harness,
            "PUT",
            "/v1/files/notes/new.md",
            &minted.basic,
            "written by the client"
        )
        .await,
        StatusCode::CREATED,
        "sync writes as well as reads"
    );
});

database_test!(an_app_password_cannot_manage_the_account, harness, {
    let minted = app_password(&harness, "kept@example.com", Role::Admin).await;

    assert_eq!(
        status(
            &harness,
            "POST",
            "/v1/app-passwords",
            &minted.basic,
            r#"{"name":"another"}"#
        )
        .await,
        StatusCode::FORBIDDEN,
        "a credential kept on disk must not mint more of itself"
    );
    assert_eq!(
        status(
            &harness,
            "PUT",
            "/v1/auth/password",
            &minted.basic,
            r#"{"current":"x","password":"a new password that is long enough"}"#,
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(&harness, "GET", "/v1/users", &minted.basic, "").await,
        StatusCode::FORBIDDEN,
        "not even an administrator's app password administers"
    );
});

database_test!(a_revoked_app_password_stops_reaching_files, harness, {
    let minted = app_password(&harness, "lost@example.com", Role::Member).await;

    revoke(&harness.state.db, minted.owner, minted.id)
        .await
        .expect("revoking");

    assert_eq!(
        status(&harness, "GET", "/v1/folders", &minted.basic, "").await,
        StatusCode::UNAUTHORIZED
    );
});

database_test!(a_wrong_secret_is_refused, harness, {
    app_password(&harness, "guess@example.com", Role::Member).await;

    assert_eq!(
        status(
            &harness,
            "GET",
            "/v1/folders",
            &basic("guess@example.com", "not-the-secret"),
            ""
        )
        .await,
        StatusCode::UNAUTHORIZED
    );
});

database_test!(a_session_still_manages_the_account, harness, {
    let owner = harness.account("session@example.com", Role::Member).await;
    let token = harness.state.sessions.issue(owner.id).expect("a token");

    assert_eq!(
        status(
            &harness,
            "POST",
            "/v1/app-passwords",
            &format!("Bearer {token}"),
            r#"{"name":"laptop"}"#
        )
        .await,
        StatusCode::CREATED
    );
});
