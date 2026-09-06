mod common;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use roxycloud_api::build_router;
use roxycloud_core::role::Role;
use tower::ServiceExt;

use common::Harness;

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

async fn call(
    harness: &Harness,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Answer {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    for (name, value) in headers {
        request = request.header(*name, *value);
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
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("reading the body")
        .to_bytes();

    Answer {
        status,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

async fn login(harness: &Harness, email: &str, password: &str) -> Answer {
    call(
        harness,
        "POST",
        "/v1/auth/login",
        &[],
        &format!(r#"{{"email":"{email}","password":"{password}"}}"#),
    )
    .await
}

async fn open(harness: &Harness, token: &str, password: &str) -> Answer {
    call(
        harness,
        "GET",
        &format!("/v1/public/{token}/content"),
        &[("x-share-password", password)],
        "",
    )
    .await
}

fn field<'a>(body: &'a str, name: &str) -> &'a str {
    body.split(&format!("\"{name}\":\""))
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no {name} in {body}"))
}

/// Mints a link on a file that exists, guarded by `password`, and answers its token.
async fn guarded_link(harness: &Harness, password: &str) -> String {
    let user = harness.account("owner@example.com", Role::Member).await;
    let bearer = harness.state.sessions.issue(user.id).expect("a token");
    harness.write(user.id, "guarded.txt", b"private").await;

    let minted = call(
        harness,
        "POST",
        "/v1/shares",
        &[(header::AUTHORIZATION.as_str(), &format!("Bearer {bearer}"))],
        &format!(r#"{{"path":"guarded.txt","password":"{password}"}}"#),
    )
    .await;
    assert_eq!(minted.status, StatusCode::CREATED, "{}", minted.body);
    field(&minted.body, "token").to_owned()
}

const FREE_ATTEMPTS: usize = 10;

database_test!(guessing_a_login_stops_being_answered, harness, {
    harness.account("target@example.com", Role::Member).await;

    for attempt in 1..=FREE_ATTEMPTS {
        let answer = login(&harness, "target@example.com", "wrong-password").await;
        assert_eq!(
            answer.status,
            StatusCode::UNAUTHORIZED,
            "attempt {attempt} should still be answered on its merits"
        );
    }
    let blocked = login(&harness, "target@example.com", "wrong-password").await;

    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        blocked
            .headers
            .get("retry-after")
            .is_some_and(|value| value.to_str().is_ok_and(|seconds| seconds != "0")),
        "a limiter a client cannot cooperate with is one it answers by retrying immediately"
    );
});

database_test!(
    the_block_holds_even_for_the_password_that_would_have_worked,
    harness,
    {
        harness.account("target@example.com", Role::Member).await;
        for _ in 1..=FREE_ATTEMPTS {
            login(&harness, "target@example.com", "wrong-password").await;
        }

        let correct = login(&harness, "target@example.com", common::PASSWORD).await;

        assert_eq!(
            correct.status,
            StatusCode::TOO_MANY_REQUESTS,
            "letting the right password through would mean hashing every guess, which is the cost \
             the limiter exists to avoid"
        );
    }
);

database_test!(
    an_address_nobody_has_is_counted_like_one_somebody_does,
    harness,
    {
        for _ in 1..=FREE_ATTEMPTS {
            login(&harness, "nobody@example.com", "wrong-password").await;
        }

        let blocked = login(&harness, "nobody@example.com", "wrong-password").await;

        assert_eq!(
            blocked.status,
            StatusCode::TOO_MANY_REQUESTS,
            "counting only real accounts would make a 429 an answer to whether an account exists"
        );
    }
);

database_test!(getting_it_right_clears_what_came_before, harness, {
    harness.account("clumsy@example.com", Role::Member).await;

    for _ in 1..FREE_ATTEMPTS {
        login(&harness, "clumsy@example.com", "wrong-password").await;
    }
    let recovered = login(&harness, "clumsy@example.com", common::PASSWORD).await;
    for _ in 1..FREE_ATTEMPTS {
        login(&harness, "clumsy@example.com", "wrong-password").await;
    }
    let still_answered = login(&harness, "clumsy@example.com", common::PASSWORD).await;

    assert_eq!(recovered.status, StatusCode::OK, "{}", recovered.body);
    assert_eq!(
        still_answered.status,
        StatusCode::OK,
        "eighteen failures either side of a success must not add up to a lockout"
    );
});

database_test!(guessing_a_share_password_stops_being_answered, harness, {
    let token = guarded_link(&harness, "sesame").await;

    for attempt in 1..=FREE_ATTEMPTS {
        let answer = open(&harness, &token, "not-it").await;
        assert_eq!(
            answer.status,
            StatusCode::UNAUTHORIZED,
            "attempt {attempt} should still be answered on its merits"
        );
    }
    let blocked = open(&harness, &token, "not-it").await;

    assert_eq!(blocked.status, StatusCode::TOO_MANY_REQUESTS);
});

database_test!(the_right_share_password_clears_what_came_before, harness, {
    let token = guarded_link(&harness, "sesame").await;

    for _ in 1..FREE_ATTEMPTS {
        open(&harness, &token, "not-it").await;
    }
    let recovered = open(&harness, &token, "sesame").await;
    for _ in 1..FREE_ATTEMPTS {
        open(&harness, &token, "not-it").await;
    }
    let still_answered = open(&harness, &token, "sesame").await;

    assert_eq!(recovered.status, StatusCode::OK);
    assert_eq!(
        still_answered.status,
        StatusCode::OK,
        "someone who keeps mistyping a link they were given must not lock themselves out of it"
    );
});

database_test!(
    a_visit_with_no_password_is_a_challenge_not_a_guess,
    harness,
    {
        let token = guarded_link(&harness, "sesame").await;

        for visitor in 1..=(FREE_ATTEMPTS + 5) {
            let answer = call(
                &harness,
                "GET",
                &format!("/v1/public/{token}/content"),
                &[],
                "",
            )
            .await;
            assert_eq!(
                answer.status,
                StatusCode::UNAUTHORIZED,
                "visitor {visitor} was turned away rather than asked for the password"
            );
        }

        assert_eq!(
            harness.attempt_rows().await,
            0,
            "a 401 is how a client learns the link wants a password, so arriving without one is not          an attempt at anything"
        );
        let known = open(&harness, &token, "sesame").await;
        assert_eq!(
            known.status,
            StatusCode::OK,
            "fifteen people opening a link in a group chat must not lock out the one who knows it"
        );
    }
);

database_test!(guesses_sent_at_once_do_not_all_get_through, harness, {
    harness.account("target@example.com", Role::Member).await;
    let burst = FREE_ATTEMPTS * 3;

    let answers = futures::future::join_all(
        (0..burst).map(|_| login(&harness, "target@example.com", "wrong-password")),
    )
    .await;

    let reached_the_password = answers
        .iter()
        .filter(|answer| answer.status == StatusCode::UNAUTHORIZED)
        .count();

    assert_eq!(
        reached_the_password, FREE_ATTEMPTS,
        "a read that decides and a write that records would let all {burst} through together,          which is how guessing is actually done"
    );
});

database_test!(
    the_table_that_limits_guessing_holds_no_working_links,
    harness,
    {
        let token = guarded_link(&harness, "sesame").await;
        open(&harness, &token, "not-it").await;

        assert_eq!(harness.attempt_rows().await, 1);
        assert!(
            !harness.attempt_subjects().await.contains(&token),
            "a limiter keyed on the raw token would be a list of live links in the clear"
        );
    }
);

database_test!(a_link_with_no_password_has_nothing_to_guess, harness, {
    let user = harness.account("owner@example.com", Role::Member).await;
    let bearer = harness.state.sessions.issue(user.id).expect("a token");
    harness.write(user.id, "open.txt", b"public").await;
    let minted = call(
        &harness,
        "POST",
        "/v1/shares",
        &[(header::AUTHORIZATION.as_str(), &format!("Bearer {bearer}"))],
        r#"{"path":"open.txt"}"#,
    )
    .await;
    let token = field(&minted.body, "token").to_owned();

    for _ in 1..=(FREE_ATTEMPTS + 5) {
        let answer = call(
            &harness,
            "GET",
            &format!("/v1/public/{token}/content"),
            &[],
            "",
        )
        .await;
        assert_eq!(answer.status, StatusCode::OK);
    }

    assert_eq!(
        harness.attempt_rows().await,
        0,
        "reading a link nobody guarded is not an attempt at anything"
    );
});

database_test!(
    a_share_password_is_not_held_to_the_account_floor,
    harness,
    {
        let user = harness.account("owner@example.com", Role::Member).await;
        let bearer = harness.state.sessions.issue(user.id).expect("a token");
        harness.write(user.id, "guarded.txt", b"private").await;
        let auth = format!("Bearer {bearer}");

        let six = call(
            &harness,
            "POST",
            "/v1/shares",
            &[(header::AUTHORIZATION.as_str(), &auth)],
            r#"{"path":"guarded.txt","password":"sesame"}"#,
        )
        .await;
        let five = call(
            &harness,
            "POST",
            "/v1/shares",
            &[(header::AUTHORIZATION.as_str(), &auth)],
            r#"{"path":"guarded.txt","password":"short"}"#,
        )
        .await;

        assert_eq!(
            six.status,
            StatusCode::CREATED,
            "a link people text to one person should not demand twelve characters: {}",
            six.body
        );
        assert_eq!(five.status, StatusCode::UNPROCESSABLE_ENTITY);
    }
);

database_test!(an_account_password_still_has_to_be_a_real_one, harness, {
    let admin = harness.account("admin@example.com", Role::Admin).await;
    let bearer = harness.state.sessions.issue(admin.id).expect("a token");

    let answer = call(
        &harness,
        "POST",
        "/v1/users",
        &[(header::AUTHORIZATION.as_str(), &format!("Bearer {bearer}"))],
        r#"{"email":"new@example.com","display_name":"New","password":"sesame"}"#,
    )
    .await;

    assert_eq!(
        answer.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "an account password is guessed offline against a stolen database, where no limiter reaches"
    );
});

database_test!(the_sweep_forgets_a_failure_nothing_followed, harness, {
    harness.account("target@example.com", Role::Member).await;
    login(&harness, "target@example.com", "wrong-password").await;
    assert_eq!(harness.attempt_rows().await, 1);

    harness.age_attempts(chrono::Duration::hours(25)).await;
    let removed = roxycloud_api::attempts::purge_expired(&harness.state.db)
        .await
        .expect("purging");

    assert_eq!(removed, 1);
    assert_eq!(harness.attempt_rows().await, 0);
});
