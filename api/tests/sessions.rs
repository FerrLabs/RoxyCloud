mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use stashden_api::build_router;
use stashden_core::role::Role;
use tower::ServiceExt;
use uuid::Uuid;

use common::{Harness, PASSWORD};

const NEW_PASSWORD: &str = "another-twelve-characters";

async fn call(harness: &Harness, method: &str, uri: &str, token: &str, body: &str) -> StatusCode {
    build_router(harness.state.clone(), &[], None)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_owned()))
                .expect("a well formed request"),
        )
        .await
        .expect("the router answers")
        .status()
}

async fn signed_in(harness: &Harness, token: &str) -> bool {
    match call(harness, "GET", "/v1/auth/me", token, "").await {
        StatusCode::OK => true,
        StatusCode::UNAUTHORIZED => false,
        other => panic!("unexpected answer from /v1/auth/me: {other}"),
    }
}

async fn change_password(harness: &Harness, token: &str, current: &str) -> StatusCode {
    call(
        harness,
        "PUT",
        "/v1/auth/password",
        token,
        &format!(r#"{{"current":"{current}","password":"{NEW_PASSWORD}"}}"#),
    )
    .await
}

async fn admin(harness: &Harness) -> String {
    let admin = harness.account("admin@example.com", Role::Admin).await;
    harness.session(admin.id).await
}

database_test!(signing_out_ends_that_session_at_once, harness, {
    let user = harness.account("leaving@example.com", Role::Member).await;
    let token = harness.session(user.id).await;
    assert!(signed_in(&harness, &token).await);

    let status = call(&harness, "POST", "/v1/auth/logout", &token, "").await;

    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        !signed_in(&harness, &token).await,
        "a copied token must stop working when its owner signs out, not twelve hours later"
    );
});

database_test!(signing_out_leaves_the_other_devices_alone, harness, {
    let user = harness.account("two@example.com", Role::Member).await;
    let laptop = harness.session(user.id).await;
    let phone = harness.session(user.id).await;

    call(&harness, "POST", "/v1/auth/logout", &laptop, "").await;

    assert!(signed_in(&harness, &phone).await);
});

database_test!(
    changing_the_password_signs_out_every_other_session,
    harness,
    {
        let user = harness.account("changed@example.com", Role::Member).await;
        let here = harness.session(user.id).await;
        let elsewhere = harness.session(user.id).await;

        let status = change_password(&harness, &here, PASSWORD).await;

        assert_eq!(status, StatusCode::NO_CONTENT);
        assert!(
            signed_in(&harness, &here).await,
            "the session that changed it stays"
        );
        assert!(
            !signed_in(&harness, &elsewhere).await,
            "a password changed because it leaked has to take the thief's session with it"
        );
    }
);

database_test!(
    a_wrong_current_password_changes_and_revokes_nothing,
    harness,
    {
        let user = harness.account("mistyped@example.com", Role::Member).await;
        let here = harness.session(user.id).await;
        let elsewhere = harness.session(user.id).await;

        let status = change_password(&harness, &here, "not-the-password").await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(signed_in(&harness, &elsewhere).await);
    }
);

database_test!(guessing_the_current_password_is_limited, harness, {
    let user = harness.account("borrowed@example.com", Role::Member).await;
    let token = harness.session(user.id).await;

    for attempt in 1..=10 {
        assert_eq!(
            change_password(&harness, &token, "a-wrong-guess").await,
            StatusCode::UNAUTHORIZED,
            "attempt {attempt} should still be answered on its merits"
        );
    }

    assert_eq!(
        change_password(&harness, &token, PASSWORD).await,
        StatusCode::TOO_MANY_REQUESTS,
        "a borrowed session must not be a way around the login limiter"
    );
});

database_test!(an_admin_reset_signs_the_account_out_everywhere, harness, {
    let admin = admin(&harness).await;
    let user = harness.account("reset@example.com", Role::Member).await;
    let token = harness.session(user.id).await;

    let status = call(
        &harness,
        "PUT",
        &format!("/v1/users/{}/password", user.id),
        &admin,
        &format!(r#"{{"password":"{NEW_PASSWORD}"}}"#),
    )
    .await;

    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!signed_in(&harness, &token).await);
});

database_test!(an_admin_can_sign_an_account_out_everywhere, harness, {
    let admin = admin(&harness).await;
    let user = harness
        .account("lost-phone@example.com", Role::Member)
        .await;
    let first = harness.session(user.id).await;
    let second = harness.session(user.id).await;

    let status = call(
        &harness,
        "DELETE",
        &format!("/v1/users/{}/sessions", user.id),
        &admin,
        "",
    )
    .await;

    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(!signed_in(&harness, &first).await);
    assert!(!signed_in(&harness, &second).await);
    assert!(signed_in(&harness, &admin).await);
});

database_test!(only_an_admin_signs_other_people_out, harness, {
    let member = harness.account("member@example.com", Role::Member).await;
    let token = harness.session(member.id).await;
    let other = harness.account("other@example.com", Role::Member).await;
    let theirs = harness.session(other.id).await;

    let status = call(
        &harness,
        "DELETE",
        &format!("/v1/users/{}/sessions", other.id),
        &token,
        "",
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(signed_in(&harness, &theirs).await);
});

database_test!(signing_out_an_unknown_account_is_not_found, harness, {
    let admin = admin(&harness).await;

    let status = call(
        &harness,
        "DELETE",
        &format!("/v1/users/{}/sessions", Uuid::now_v7()),
        &admin,
        "",
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
});

database_test!(
    enabling_an_account_again_does_not_revive_its_sessions,
    harness,
    {
        let admin = admin(&harness).await;
        let user = harness.account("paused@example.com", Role::Member).await;
        let token = harness.session(user.id).await;

        call(
            &harness,
            "POST",
            &format!("/v1/users/{}/disable", user.id),
            &admin,
            "",
        )
        .await;
        call(
            &harness,
            "POST",
            &format!("/v1/users/{}/enable", user.id),
            &admin,
            "",
        )
        .await;

        assert!(
            !signed_in(&harness, &token).await,
            "a session from before the account was disabled belongs to whoever it was disabled over"
        );
    }
);

database_test!(a_token_for_a_session_that_is_gone_is_refused, harness, {
    let user = harness.account("purged@example.com", Role::Member).await;
    let token = harness.session(user.id).await;

    sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(user.id)
        .execute(&harness.state.db)
        .await
        .expect("dropping the session");

    assert!(!signed_in(&harness, &token).await);
});
