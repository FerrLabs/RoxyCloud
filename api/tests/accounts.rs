mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::Value;
use stashden_api::build_router;
use stashden_api::recovery;
use stashden_core::role::Role;
use stashden_core::user::Email;
use tower::ServiceExt;
use uuid::Uuid;

use common::{Harness, PASSWORD};

const NEW_PASSWORD: &str = "a-password-after-recovery";

async fn call(
    harness: &Harness,
    method: &str,
    uri: &str,
    authorization: Option<&str>,
    body: &str,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(authorization) = authorization {
        request = request.header(header::AUTHORIZATION, authorization);
    }
    let response = build_router(harness.state.clone(), &[], None)
        .oneshot(
            request
                .body(Body::from(body.to_owned()))
                .expect("a well formed request"),
        )
        .await
        .expect("the router answers");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("a body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn member(harness: &Harness, email: &str, role: Role) -> (Uuid, String) {
    let user = harness.account(email, role).await;
    let token = harness.session(user.id).await;
    (user.id, format!("Bearer {token}"))
}

async fn login(harness: &Harness, email: &str, password: &str) -> StatusCode {
    call(
        harness,
        "POST",
        "/v1/auth/login",
        None,
        &format!(r#"{{"email":"{email}","password":"{password}"}}"#),
    )
    .await
    .0
}

database_test!(a_person_changes_their_own_display_name, harness, {
    let (_, bearer) = member(&harness, "renamed@example.com", Role::Reader).await;

    let (status, user) = call(
        &harness,
        "PUT",
        "/v1/auth/me",
        Some(&bearer),
        r#"{"display_name":"  Ada Lovelace  "}"#,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{user}");
    assert_eq!(user["display_name"], "Ada Lovelace");
    let (_, me) = call(&harness, "GET", "/v1/auth/me", Some(&bearer), "").await;
    assert_eq!(me["display_name"], "Ada Lovelace");
});

database_test!(a_display_name_has_to_be_something, harness, {
    let (_, bearer) = member(&harness, "blank@example.com", Role::Member).await;
    let too_long = "x".repeat(101);

    let (blank, _) = call(
        &harness,
        "PUT",
        "/v1/auth/me",
        Some(&bearer),
        r#"{"display_name":"   "}"#,
    )
    .await;
    let (long, _) = call(
        &harness,
        "PUT",
        "/v1/auth/me",
        Some(&bearer),
        &format!(r#"{{"display_name":"{too_long}"}}"#),
    )
    .await;

    assert_eq!(blank, StatusCode::BAD_REQUEST);
    assert_eq!(long, StatusCode::BAD_REQUEST);
});

database_test!(
    an_admin_changes_an_email_and_the_old_one_stops_working,
    harness,
    {
        let (_, admin) = member(&harness, "admin@example.com", Role::Admin).await;
        let (id, _) = member(&harness, "before@example.com", Role::Member).await;

        let (status, user) = call(
            &harness,
            "PUT",
            &format!("/v1/users/{id}/email"),
            Some(&admin),
            r#"{"email":"After@Example.com"}"#,
        )
        .await;

        assert_eq!(status, StatusCode::OK, "{user}");
        assert_eq!(user["email"], "after@example.com");
        assert_eq!(
            login(&harness, "after@example.com", PASSWORD).await,
            StatusCode::OK
        );
        assert_eq!(
            login(&harness, "before@example.com", PASSWORD).await,
            StatusCode::UNAUTHORIZED
        );
    }
);

database_test!(an_email_that_belongs_to_someone_else_is_refused, harness, {
    let (_, admin) = member(&harness, "admin@example.com", Role::Admin).await;
    let (id, _) = member(&harness, "mine@example.com", Role::Member).await;
    harness.account("theirs@example.com", Role::Member).await;

    let (status, _) = call(
        &harness,
        "PUT",
        &format!("/v1/users/{id}/email"),
        Some(&admin),
        r#"{"email":"theirs@example.com"}"#,
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        login(&harness, "mine@example.com", PASSWORD).await,
        StatusCode::OK
    );
});

database_test!(a_person_cannot_give_themselves_an_email, harness, {
    let (id, bearer) = member(&harness, "stuck@example.com", Role::Member).await;

    let (status, _) = call(
        &harness,
        "PUT",
        &format!("/v1/users/{id}/email"),
        Some(&bearer),
        r#"{"email":"someone-else@example.com"}"#,
    )
    .await;

    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an unverified address is how somebody else's sign-in lands in the wrong account"
    );
});

database_test!(shares_follow_the_account_to_its_new_address, harness, {
    let (owner, owner_bearer) = member(&harness, "owner@example.com", Role::Member).await;
    let (_, admin) = member(&harness, "admin@example.com", Role::Admin).await;
    let (guest_id, guest) = member(&harness, "guest@example.com", Role::Member).await;
    harness.write(owner, "photos/beach.jpg", b"sand").await;
    let (granted, _) = call(
        &harness,
        "POST",
        "/v1/grants",
        Some(&owner_bearer),
        r#"{"path":"photos","email":"guest@example.com","access":"read"}"#,
    )
    .await;
    assert_eq!(granted, StatusCode::CREATED);

    call(
        &harness,
        "PUT",
        &format!("/v1/users/{guest_id}/email"),
        Some(&admin),
        r#"{"email":"renamed-guest@example.com"}"#,
    )
    .await;

    let (status, listing) = call(
        &harness,
        "GET",
        "/v1/folders/Shared%20with%20me/photos",
        Some(&guest),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listing}");
});

database_test!(
    recovery_sets_a_password_and_signs_everything_out,
    harness,
    {
        let user = harness.account("sole-admin@example.com", Role::Admin).await;
        let token = harness.session(user.id).await;
        let email: Email = "sole-admin@example.com".parse().expect("an email");
        for _ in 0..12 {
            login(&harness, "sole-admin@example.com", "not-the-password").await;
        }
        assert_eq!(
            login(&harness, "sole-admin@example.com", PASSWORD).await,
            StatusCode::TOO_MANY_REQUESTS,
            "locked out by the limiter"
        );

        recovery::reset_password(&harness.state.db, &email, NEW_PASSWORD)
            .await
            .expect("recovering");

        assert_eq!(
            login(&harness, "sole-admin@example.com", NEW_PASSWORD).await,
            StatusCode::OK,
            "the lockout is cleared and the new password works"
        );
        let (status, _) = call(
            &harness,
            "GET",
            "/v1/auth/me",
            Some(&format!("Bearer {token}")),
            "",
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "the old session is gone");
    }
);

database_test!(
    recovery_refuses_a_password_that_is_too_weak_and_an_unknown_account,
    harness,
    {
        harness.account("someone@example.com", Role::Admin).await;
        let known: Email = "someone@example.com".parse().expect("an email");
        let unknown: Email = "nobody@example.com".parse().expect("an email");

        let weak = recovery::reset_password(&harness.state.db, &known, "short").await;
        let missing = recovery::reset_password(&harness.state.db, &unknown, NEW_PASSWORD).await;

        assert!(weak.is_err());
        assert!(missing.is_err());
        assert_eq!(
            login(&harness, "someone@example.com", PASSWORD).await,
            StatusCode::OK,
            "a refused reset changes nothing"
        );
    }
);
