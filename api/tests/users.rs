mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use roxycloud_api::build_router;
use roxycloud_core::role::Role;
use tower::ServiceExt;
use uuid::Uuid;

use common::Harness;

struct Answer {
    status: StatusCode,
    body: String,
}

async fn call(harness: &Harness, method: &str, path: &str, token: &str, body: &str) -> Answer {
    let response = build_router(harness.state.clone(), &[], None)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_owned()))
                .expect("a well formed request"),
        )
        .await
        .expect("the router answers");

    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("reading the body")
        .to_bytes();

    Answer {
        status,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

async fn session(harness: &Harness, email: &str, role: Role) -> (Uuid, String) {
    let user = harness.account(email, role).await;
    let token = harness.state.sessions.issue(user.id).expect("a token");
    (user.id, token)
}

fn id_in(body: &str) -> String {
    body.split("\"id\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("an id in the response")
        .to_owned()
}

const NEW_ACCOUNT: &str = r#"{"email":"new@example.com","display_name":"New Person","password":"twelve-characters-at-least"}"#;

database_test!(
    an_administrator_creates_an_account_that_can_log_in,
    harness,
    {
        let (_, admin) = session(&harness, "admin@example.com", Role::Admin).await;

        let created = call(&harness, "POST", "/v1/users", &admin, NEW_ACCOUNT).await;

        assert_eq!(created.status, StatusCode::CREATED);
        assert!(created.body.contains("new@example.com"), "{}", created.body);
        assert!(
            !created.body.contains("password_hash"),
            "the hash is not something an API hands back: {}",
            created.body
        );

        let listed = call(&harness, "GET", "/v1/users", &admin, "").await;
        assert!(listed.body.contains("new@example.com"), "{}", listed.body);
    }
);

database_test!(everything_here_is_refused_to_everyone_else, harness, {
    let (target, _) = session(&harness, "target@example.com", Role::Member).await;
    let (_, member) = session(&harness, "member@example.com", Role::Member).await;
    let (_, reader) = session(&harness, "reader@example.com", Role::Reader).await;

    for token in [&member, &reader] {
        for (method, path, body) in [
            ("POST", "/v1/users".to_owned(), NEW_ACCOUNT),
            ("GET", "/v1/users".to_owned(), ""),
            ("POST", format!("/v1/users/{target}/disable"), ""),
            ("POST", format!("/v1/users/{target}/enable"), ""),
            (
                "PUT",
                format!("/v1/users/{target}/role"),
                r#"{"role":"admin"}"#,
            ),
            (
                "PUT",
                format!("/v1/users/{target}/quota"),
                r#"{"bytes_max":1}"#,
            ),
            (
                "PUT",
                format!("/v1/users/{target}/password"),
                r#"{"password":"twelve-characters-at-least"}"#,
            ),
        ] {
            let answer = call(&harness, method, &path, token, body).await;
            assert_eq!(
                answer.status,
                StatusCode::FORBIDDEN,
                "{method} {path} was not refused"
            );
        }
    }
});

database_test!(disabling_an_account_ends_its_sessions_at_once, harness, {
    let (_, admin) = session(&harness, "admin@example.com", Role::Admin).await;
    let (id, theirs) = session(&harness, "disabled@example.com", Role::Member).await;

    let before = call(&harness, "GET", "/v1/folders", &theirs, "").await;
    assert_eq!(before.status, StatusCode::OK);

    let disabled = call(
        &harness,
        "POST",
        &format!("/v1/users/{id}/disable"),
        &admin,
        "",
    )
    .await;
    assert_eq!(disabled.status, StatusCode::OK);

    let after = call(&harness, "GET", "/v1/folders", &theirs, "").await;
    assert_eq!(
        after.status,
        StatusCode::UNAUTHORIZED,
        "a token outliving the account it names is the whole point of disabling one"
    );
});

database_test!(re_enabling_an_account_lets_it_back_in, harness, {
    let (_, admin) = session(&harness, "admin@example.com", Role::Admin).await;
    let (id, theirs) = session(&harness, "returning@example.com", Role::Member).await;
    call(
        &harness,
        "POST",
        &format!("/v1/users/{id}/disable"),
        &admin,
        "",
    )
    .await;

    call(
        &harness,
        "POST",
        &format!("/v1/users/{id}/enable"),
        &admin,
        "",
    )
    .await;

    let after = call(&harness, "GET", "/v1/folders", &theirs, "").await;
    assert_eq!(
        after.status,
        StatusCode::OK,
        "the token was never revoked, only the account it names was"
    );
});

database_test!(an_administrator_cannot_lock_themselves_out, harness, {
    let (id, admin) = session(&harness, "only@example.com", Role::Admin).await;

    let disabled = call(
        &harness,
        "POST",
        &format!("/v1/users/{id}/disable"),
        &admin,
        "",
    )
    .await;
    let demoted = call(
        &harness,
        "PUT",
        &format!("/v1/users/{id}/role"),
        &admin,
        r#"{"role":"reader"}"#,
    )
    .await;

    assert_eq!(disabled.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        demoted.status,
        StatusCode::BAD_REQUEST,
        "an installation nobody can administer is not a state an API should let you reach"
    );
    let still = call(&harness, "GET", "/v1/users", &admin, "").await;
    assert_eq!(still.status, StatusCode::OK);
});

database_test!(a_role_change_takes_effect_on_the_next_request, harness, {
    let (_, admin) = session(&harness, "admin@example.com", Role::Admin).await;
    let (id, theirs) = session(&harness, "promoted@example.com", Role::Reader).await;

    let before = call(&harness, "PUT", "/v1/files/a.txt", &theirs, "denied").await;
    call(
        &harness,
        "PUT",
        &format!("/v1/users/{id}/role"),
        &admin,
        r#"{"role":"member"}"#,
    )
    .await;
    let after = call(&harness, "PUT", "/v1/files/a.txt", &theirs, "allowed").await;

    assert_eq!(before.status, StatusCode::FORBIDDEN);
    assert_eq!(after.status, StatusCode::CREATED);
});

database_test!(
    a_quota_can_be_set_before_the_account_has_written_anything,
    harness,
    {
        let (_, admin) = session(&harness, "admin@example.com", Role::Admin).await;
        let (id, theirs) = session(&harness, "capped@example.com", Role::Member).await;

        let set = call(
            &harness,
            "PUT",
            &format!("/v1/users/{id}/quota"),
            &admin,
            r#"{"bytes_max":10}"#,
        )
        .await;
        assert_eq!(set.status, StatusCode::NO_CONTENT);

        let refused = call(
            &harness,
            "PUT",
            "/v1/files/big.txt",
            &theirs,
            "more than ten bytes",
        )
        .await;

        assert_eq!(
            refused.status,
            StatusCode::INSUFFICIENT_STORAGE,
            "a quota set before the first write has to survive the row being created"
        );
        assert_eq!(harness.used_bytes(id).await, 0);
    }
);

database_test!(a_quota_of_nothing_is_refused_rather_than_stored, harness, {
    let (_, admin) = session(&harness, "admin@example.com", Role::Admin).await;
    let (id, _) = session(&harness, "zero@example.com", Role::Member).await;

    let answer = call(
        &harness,
        "PUT",
        &format!("/v1/users/{id}/quota"),
        &admin,
        r#"{"bytes_max":0}"#,
    )
    .await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
});

database_test!(a_password_change_needs_the_current_one, harness, {
    let (_, theirs) = session(&harness, "changing@example.com", Role::Member).await;

    let wrong = call(
        &harness,
        "PUT",
        "/v1/auth/password",
        &theirs,
        r#"{"current":"not the password","password":"a-brand-new-password"}"#,
    )
    .await;
    let right = call(
        &harness,
        "PUT",
        "/v1/auth/password",
        &theirs,
        &format!(
            r#"{{"current":"{}","password":"a-brand-new-password"}}"#,
            common::PASSWORD
        ),
    )
    .await;

    assert_eq!(
        wrong.status,
        StatusCode::UNAUTHORIZED,
        "a borrowed session must not be enough to lock the owner out"
    );
    assert_eq!(right.status, StatusCode::NO_CONTENT);
});

database_test!(a_new_password_has_to_be_a_password, harness, {
    let (_, theirs) = session(&harness, "weak@example.com", Role::Member).await;

    let answer = call(
        &harness,
        "PUT",
        "/v1/auth/password",
        &theirs,
        &format!(r#"{{"current":"{}","password":"short"}}"#, common::PASSWORD),
    )
    .await;

    assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY);
});

database_test!(
    an_administrator_resets_a_password_without_knowing_it,
    harness,
    {
        let (_, admin) = session(&harness, "admin@example.com", Role::Admin).await;
        let created = call(&harness, "POST", "/v1/users", &admin, NEW_ACCOUNT).await;
        let id = id_in(&created.body);

        let reset = call(
            &harness,
            "PUT",
            &format!("/v1/users/{id}/password"),
            &admin,
            r#"{"password":"the-replacement-password"}"#,
        )
        .await;

        assert_eq!(reset.status, StatusCode::NO_CONTENT);
        let signed_in = call(
            &harness,
            "POST",
            "/v1/auth/login",
            "",
            r#"{"email":"new@example.com","password":"the-replacement-password"}"#,
        )
        .await;
        assert_eq!(signed_in.status, StatusCode::OK, "{}", signed_in.body);
    }
);

database_test!(an_account_that_exists_is_not_created_twice, harness, {
    let (_, admin) = session(&harness, "admin@example.com", Role::Admin).await;
    call(&harness, "POST", "/v1/users", &admin, NEW_ACCOUNT).await;

    let again = call(&harness, "POST", "/v1/users", &admin, NEW_ACCOUNT).await;

    assert_eq!(again.status, StatusCode::CONFLICT);
});

database_test!(an_account_that_is_not_there_is_not_found, harness, {
    let (_, admin) = session(&harness, "admin@example.com", Role::Admin).await;
    let nobody = Uuid::now_v7();

    for (method, path, body) in [
        ("POST", format!("/v1/users/{nobody}/disable"), ""),
        (
            "PUT",
            format!("/v1/users/{nobody}/role"),
            r#"{"role":"member"}"#,
        ),
        (
            "PUT",
            format!("/v1/users/{nobody}/quota"),
            r#"{"bytes_max":100}"#,
        ),
        (
            "PUT",
            format!("/v1/users/{nobody}/password"),
            r#"{"password":"twelve-characters-at-least"}"#,
        ),
    ] {
        let answer = call(&harness, method, &path, &admin, body).await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{method} {path}");
    }
});
