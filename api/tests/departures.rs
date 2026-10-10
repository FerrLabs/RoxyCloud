mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::Value;
use stashden_api::build_router;
use stashden_core::role::Role;
use tower::ServiceExt;
use uuid::Uuid;

use common::Harness;

async fn call(
    harness: &Harness,
    method: &str,
    uri: &str,
    bearer: &str,
    body: &str,
) -> (StatusCode, Value) {
    let response = build_router(harness.state.clone(), &[], None)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, bearer)
                .header(header::CONTENT_TYPE, "application/json")
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

async fn person(harness: &Harness, email: &str, role: Role) -> (Uuid, String) {
    let user = harness.account(email, role).await;
    let token = harness.session(user.id).await;
    (user.id, format!("Bearer {token}"))
}

async fn names(harness: &Harness, uri: &str, bearer: &str) -> Vec<String> {
    let (status, listing) = call(harness, "GET", uri, bearer, "").await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    listing
        .as_array()
        .expect("a listing")
        .iter()
        .map(|node| node["name"].as_str().expect("a name").to_owned())
        .collect()
}

database_test!(a_deleted_account_cannot_sign_in_again, harness, {
    let (_, admin) = person(&harness, "admin@example.com", Role::Admin).await;
    let (id, leaving) = person(&harness, "leaving@example.com", Role::Member).await;

    let (status, _) = call(&harness, "DELETE", &format!("/v1/users/{id}"), &admin, "").await;

    assert_eq!(status, StatusCode::NO_CONTENT);
    let (after, _) = call(&harness, "GET", "/v1/auth/me", &leaving, "").await;
    assert_eq!(after, StatusCode::UNAUTHORIZED);
    assert_eq!(harness.account_count().await, 1);
});

database_test!(
    without_a_hand_over_the_files_are_released_for_collection,
    harness,
    {
        let (_, admin) = person(&harness, "admin@example.com", Role::Admin).await;
        let (id, _) = person(&harness, "gone@example.com", Role::Member).await;
        let node = harness
            .write(id, "docs/plan.md", b"the plan of someone leaving")
            .await;
        harness
            .write(id, "docs/plan.md", b"the plan, second draft")
            .await;
        let hash = node.blob_hash.expect("a file");
        let kept = blake3::hash(b"the plan, second draft").into();
        assert_eq!(harness.blob(hash).await.map(|(count, _)| count), Some(1));

        let (status, _) = call(&harness, "DELETE", &format!("/v1/users/{id}"), &admin, "").await;

        assert_eq!(status, StatusCode::NO_CONTENT);
        assert_eq!(
            harness.blob(hash).await,
            Some((0, true)),
            "the old version's bytes are unreferenced and wait for the sweep"
        );
        assert_eq!(harness.blob(kept).await, Some((0, true)));
    }
);

database_test!(a_hand_over_gives_the_files_to_someone_else, harness, {
    let (_, admin) = person(&harness, "admin@example.com", Role::Admin).await;
    let (leaving, _) = person(&harness, "ada@example.com", Role::Member).await;
    let (heir, heir_bearer) = person(&harness, "grace@example.com", Role::Member).await;
    harness.write(leaving, "notes/a.txt", b"kept by ada").await;
    harness.write(leaving, "b.txt", b"also ada's").await;
    harness.write(heir, "own.txt", b"grace's own").await;
    let before = harness.used_bytes(heir).await;
    let handed = harness.used_bytes(leaving).await;
    assert!(handed > 0);

    let (status, body) = call(
        &harness,
        "DELETE",
        &format!("/v1/users/{leaving}?hand_over_to={heir}"),
        &admin,
        "",
    )
    .await;

    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let top = names(&harness, "/v1/folders", &heir_bearer).await;
    assert!(top.contains(&"own.txt".to_owned()), "{top:?}");
    let folder = top
        .iter()
        .find(|name| name.contains("ada@example.com"))
        .expect("a folder named after the person who left")
        .clone();
    let inside = names(
        &harness,
        &format!(
            "/v1/folders/{}",
            folder
                .replace(' ', "%20")
                .replace('(', "%28")
                .replace(')', "%29")
        ),
        &heir_bearer,
    )
    .await;
    assert_eq!(inside.len(), 2, "{inside:?}");
    assert_eq!(harness.used_bytes(heir).await, before + handed);
});

database_test!(
    a_hand_over_that_does_not_fit_the_recipients_quota_changes_nothing,
    harness,
    {
        let (_, admin) = person(&harness, "admin@example.com", Role::Admin).await;
        let (leaving, _) = person(&harness, "big@example.com", Role::Member).await;
        let (heir, _) = person(&harness, "small@example.com", Role::Member).await;
        harness.write(leaving, "big.bin", &[1; 500]).await;
        harness.root(heir).await;
        harness.set_quota(heir, 100).await;

        let (status, _) = call(
            &harness,
            "DELETE",
            &format!("/v1/users/{leaving}?hand_over_to={heir}"),
            &admin,
            "",
        )
        .await;

        assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE);
        assert_eq!(
            harness.account_count().await,
            3,
            "the account is still there"
        );
        assert_eq!(harness.used_bytes(leaving).await, 500);
    }
);

database_test!(an_admin_cannot_delete_themselves, harness, {
    let (id, admin) = person(&harness, "only@example.com", Role::Admin).await;

    let (status, _) = call(&harness, "DELETE", &format!("/v1/users/{id}"), &admin, "").await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(harness.account_count().await, 1);
});

database_test!(a_member_cannot_delete_anyone, harness, {
    let (_, member) = person(&harness, "member@example.com", Role::Member).await;
    let (other, _) = person(&harness, "other@example.com", Role::Member).await;

    let (status, _) = call(
        &harness,
        "DELETE",
        &format!("/v1/users/{other}"),
        &member,
        "",
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
});

database_test!(
    files_cannot_be_handed_to_a_disabled_account_or_a_reader,
    harness,
    {
        let (_, admin) = person(&harness, "admin@example.com", Role::Admin).await;
        let (leaving, _) = person(&harness, "leaving@example.com", Role::Member).await;
        let (disabled, _) = person(&harness, "disabled@example.com", Role::Member).await;
        let (reader, _) = person(&harness, "reader@example.com", Role::Reader).await;
        harness.write(leaving, "a.txt", b"hello").await;
        call(
            &harness,
            "POST",
            &format!("/v1/users/{disabled}/disable"),
            &admin,
            "",
        )
        .await;

        for recipient in [disabled, reader] {
            let (status, _) = call(
                &harness,
                "DELETE",
                &format!("/v1/users/{leaving}?hand_over_to={recipient}"),
                &admin,
                "",
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        assert_eq!(harness.account_count().await, 4);
    }
);

database_test!(
    files_cannot_be_handed_to_the_account_being_deleted,
    harness,
    {
        let (_, admin) = person(&harness, "admin@example.com", Role::Admin).await;
        let (id, _) = person(&harness, "loop@example.com", Role::Member).await;

        let (status, _) = call(
            &harness,
            "DELETE",
            &format!("/v1/users/{id}?hand_over_to={id}"),
            &admin,
            "",
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
);

database_test!(shares_made_by_a_deleted_account_go_with_it, harness, {
    let (_, admin) = person(&harness, "admin@example.com", Role::Admin).await;
    let (owner, owner_bearer) = person(&harness, "owner@example.com", Role::Member).await;
    let (_, guest) = person(&harness, "guest@example.com", Role::Member).await;
    harness.write(owner, "photos/beach.jpg", b"sand").await;
    let (granted, _) = call(
        &harness,
        "POST",
        "/v1/grants",
        &owner_bearer,
        r#"{"path":"photos","email":"guest@example.com","access":"read"}"#,
    )
    .await;
    assert_eq!(granted, StatusCode::CREATED);

    call(
        &harness,
        "DELETE",
        &format!("/v1/users/{owner}"),
        &admin,
        "",
    )
    .await;

    let (status, _) = call(
        &harness,
        "GET",
        "/v1/folders/Shared%20with%20me/photos",
        &guest,
        "",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
});

database_test!(
    a_name_already_taken_in_the_recipients_files_is_not_merged_into,
    harness,
    {
        let (_, admin) = person(&harness, "admin@example.com", Role::Admin).await;
        let (leaving, _) = person(&harness, "ada@example.com", Role::Member).await;
        let (heir, heir_bearer) = person(&harness, "grace@example.com", Role::Member).await;
        harness.write(leaving, "a.txt", b"from ada").await;
        harness
            .write(heir, "Tester (ada@example.com)/keep.txt", b"already here")
            .await;

        let (status, _) = call(
            &harness,
            "DELETE",
            &format!("/v1/users/{leaving}?hand_over_to={heir}"),
            &admin,
            "",
        )
        .await;

        assert_eq!(status, StatusCode::NO_CONTENT);
        let top = names(&harness, "/v1/folders", &heir_bearer).await;
        assert_eq!(
            top,
            ["Tester (ada@example.com)", "Tester (ada@example.com) 2"],
            "{top:?}"
        );
        let untouched = names(
            &harness,
            "/v1/folders/Tester%20%28ada%40example.com%29",
            &heir_bearer,
        )
        .await;
        assert_eq!(untouched, ["keep.txt"]);
    }
);

database_test!(
    a_long_non_latin_display_name_does_not_block_a_hand_over,
    harness,
    {
        let (_, admin) = person(&harness, "admin@example.com", Role::Admin).await;
        let (leaving, _) = person(&harness, "long@example.com", Role::Member).await;
        let (heir, heir_bearer) = person(&harness, "grace@example.com", Role::Member).await;
        harness
            .write(leaving, "a.txt", b"from the one with a long name")
            .await;
        sqlx::query("UPDATE users SET display_name = $2 WHERE id = $1")
            .bind(leaving)
            .bind("\u{8a9e}".repeat(100))
            .execute(&harness.state.db)
            .await
            .expect("a hundred characters, which the display name limit allows");

        let (status, body) = call(
            &harness,
            "DELETE",
            &format!("/v1/users/{leaving}?hand_over_to={heir}"),
            &admin,
            "",
        )
        .await;

        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
        let top = names(&harness, "/v1/folders", &heir_bearer).await;
        assert_eq!(top.len(), 1, "{top:?}");
        assert!(top[0].len() <= 255, "{} bytes", top[0].len());
        assert!(top[0].starts_with('\u{8a9e}'));
    }
);
