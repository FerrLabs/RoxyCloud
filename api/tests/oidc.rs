mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use roxycloud_api::build_router;
use roxycloud_api::oidc::{self, Claims, Refused, RefusedOrFailed};
use roxycloud_core::role::Role;
use tower::ServiceExt;

use common::Harness;

fn claims(email: &str, verified: bool) -> Claims {
    Claims {
        sub: "provider-subject".to_owned(),
        iss: "https://idp.example".to_owned(),
        email: Some(email.to_owned()),
        email_verified: verified,
        name: Some("Someone Else".to_owned()),
    }
}

fn refusal(outcome: Result<roxycloud_core::user::User, RefusedOrFailed>) -> Refused {
    match outcome {
        Err(RefusedOrFailed::Refused(refused)) => refused,
        Err(RefusedOrFailed::Failed(failure)) => panic!("failed rather than refused: {failure}"),
        Ok(user) => panic!("admitted {} when it should have been refused", user.email),
    }
}

async fn call(
    harness: &Harness,
    method: &str,
    path: &str,
    bearer: &str,
    body: &str,
) -> (StatusCode, String) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if !bearer.is_empty() {
        request = request.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
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
    let body = response
        .into_body()
        .collect()
        .await
        .expect("reading the body")
        .to_bytes();

    (status, String::from_utf8_lossy(&body).into_owned())
}

database_test!(
    an_unverified_address_never_takes_over_an_account,
    harness,
    {
        let existing = harness.account("victim@example.com", Role::Admin).await;

        let outcome = oidc::admit(
            &harness.state.db,
            &claims("victim@example.com", false),
            false,
        )
        .await;

        assert_eq!(
            refusal(outcome),
            Refused::Unverified,
            "asserting somebody else's address at a provider that never checked it is how these \
             integrations are broken"
        );
        // And the account it aimed at is untouched.
        let after = harness.account_by_email("victim@example.com").await;
        assert_eq!(after.id, existing.id);
        assert_eq!(after.role, Role::Admin);
    }
);

database_test!(an_unverified_address_never_creates_an_account, harness, {
    let outcome = oidc::admit(
        &harness.state.db,
        &claims("stranger@example.com", false),
        true,
    )
    .await;

    assert_eq!(refusal(outcome), Refused::Unverified);
    assert_eq!(
        harness.account_count().await,
        0,
        "creation is allowed here, and the verification is what stops it anyway"
    );
});

database_test!(a_verified_address_reaches_the_account_it_names, harness, {
    let existing = harness.account("person@example.com", Role::Member).await;

    let admitted = oidc::admit(
        &harness.state.db,
        &claims("person@example.com", true),
        false,
    )
    .await
    .expect("admitted");

    assert_eq!(admitted.id, existing.id);
});

database_test!(
    a_verified_stranger_is_created_only_where_that_is_configured,
    harness,
    {
        let refused = oidc::admit(&harness.state.db, &claims("new@example.com", true), false).await;
        assert_eq!(refusal(refused), Refused::NoAccount);
        assert_eq!(harness.account_count().await, 0);

        let created = oidc::admit(&harness.state.db, &claims("new@example.com", true), true)
            .await
            .expect("created");

        assert_eq!(created.email.as_str(), "new@example.com");
        assert_eq!(
            created.role,
            Role::Member,
            "arriving through a provider is not a reason to be an administrator"
        );
    }
);

database_test!(
    an_account_made_through_a_provider_has_no_password_to_guess,
    harness,
    {
        oidc::admit(&harness.state.db, &claims("new@example.com", true), true)
            .await
            .expect("created");

        let (status, _) = call(
            &harness,
            "POST",
            "/v1/auth/login",
            "",
            r#"{"email":"new@example.com","password":""}"#,
        )
        .await;

        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
);

database_test!(a_spent_flow_cannot_be_spent_again, harness, {
    let started = oidc::start();
    oidc::remember(&harness.state.db, &started)
        .await
        .expect("remembered");

    let first = oidc::claim_flow(&harness.state.db, &started.state).await;
    let second = oidc::claim_flow(&harness.state.db, &started.state).await;

    assert_eq!(first.expect("the verifier"), started.verifier);
    assert!(
        second.is_err(),
        "a state that can be spent twice is a code that can be replayed"
    );
});

database_test!(an_expired_flow_is_not_worth_a_verifier, harness, {
    let started = oidc::start();
    oidc::remember(&harness.state.db, &started)
        .await
        .expect("remembered");
    harness.expire_oidc_flows().await;

    assert!(
        oidc::claim_flow(&harness.state.db, &started.state)
            .await
            .is_err()
    );
    assert_eq!(
        oidc::purge_expired(&harness.state.db)
            .await
            .expect("purging"),
        1
    );
});

database_test!(password_login_can_be_turned_off_and_back_on, harness, {
    let admin = harness.account("admin@example.com", Role::Admin).await;
    let bearer = harness.state.sessions.issue(admin.id).expect("a token");

    let before = call(
        &harness,
        "POST",
        "/v1/auth/login",
        "",
        &format!(
            r#"{{"email":"admin@example.com","password":"{}"}}"#,
            common::PASSWORD
        ),
    )
    .await;

    // With no provider configured there would be no way back in, so this is refused.
    let refused = call(
        &harness,
        "PUT",
        "/v1/auth/methods",
        &bearer,
        r#"{"password":false}"#,
    )
    .await;

    assert_eq!(before.0, StatusCode::OK);
    assert_eq!(
        refused.0,
        StatusCode::BAD_REQUEST,
        "turning off the only way in is not a setting anybody meant to change"
    );
});

database_test!(the_sign_in_screen_is_told_what_it_may_offer, harness, {
    let (status, body) = call(&harness, "GET", "/v1/auth/methods", "", "").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"password\":true"), "{body}");
    assert!(
        body.contains("\"oidc\":false"),
        "nothing is configured in the harness: {body}"
    );
});

database_test!(only_an_administrator_changes_how_people_sign_in, harness, {
    let member = harness.account("member@example.com", Role::Member).await;
    let bearer = harness.state.sessions.issue(member.id).expect("a token");

    let (status, _) = call(
        &harness,
        "PUT",
        "/v1/auth/methods",
        &bearer,
        r#"{"password":true}"#,
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
});
