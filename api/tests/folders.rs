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
    body: &[u8],
) -> (StatusCode, Value) {
    let response = build_router(harness.state.clone(), &[], None)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, bearer)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_vec()))
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

async fn names(harness: &Harness, uri: &str, bearer: &str) -> Vec<String> {
    let (status, listing) = call(harness, "GET", uri, bearer, b"").await;
    assert_eq!(status, StatusCode::OK, "{listing}");
    listing
        .as_array()
        .expect("a listing")
        .iter()
        .map(|node| node["name"].as_str().expect("a name").to_owned())
        .collect()
}

database_test!(a_folder_is_created_and_listed, harness, {
    let (_, bearer) = member(&harness, "maker@example.com", Role::Member).await;

    let (status, created) = call(&harness, "POST", "/v1/folders/photos", &bearer, b"").await;

    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["name"], "photos");
    assert_eq!(created["kind"], "directory");
    assert_eq!(names(&harness, "/v1/folders", &bearer).await, ["photos"]);
});

database_test!(
    a_folder_is_created_inside_another_and_makes_the_ones_above,
    harness,
    {
        let (_, bearer) = member(&harness, "nested@example.com", Role::Member).await;
        call(&harness, "POST", "/v1/folders/photos", &bearer, b"").await;

        let (status, _) = call(
            &harness,
            "POST",
            "/v1/folders/photos/2026/summer",
            &bearer,
            b"",
        )
        .await;

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(
            names(&harness, "/v1/folders/photos/2026", &bearer).await,
            ["summer"]
        );
    }
);

database_test!(
    a_name_that_is_taken_is_a_conflict_not_a_second_folder,
    harness,
    {
        let (owner, bearer) = member(&harness, "taken@example.com", Role::Member).await;
        harness.write(owner, "notes", b"a file").await;
        call(&harness, "POST", "/v1/folders/docs", &bearer, b"").await;

        let (over_file, _) = call(&harness, "POST", "/v1/folders/notes", &bearer, b"").await;
        let (over_folder, _) = call(&harness, "POST", "/v1/folders/docs", &bearer, b"").await;

        assert_eq!(over_file, StatusCode::CONFLICT);
        assert_eq!(over_folder, StatusCode::CONFLICT);
    }
);

database_test!(a_reader_cannot_create_a_folder, harness, {
    let (_, bearer) = member(&harness, "reader@example.com", Role::Reader).await;

    let (status, _) = call(&harness, "POST", "/v1/folders/nope", &bearer, b"").await;

    assert_eq!(status, StatusCode::FORBIDDEN);
});

database_test!(the_root_and_the_shelf_name_cannot_be_made, harness, {
    let (_, bearer) = member(&harness, "reserved@example.com", Role::Member).await;

    let (root, _) = call(&harness, "POST", "/v1/folders", &bearer, b"").await;
    let (shelf, _) = call(
        &harness,
        "POST",
        "/v1/folders/Shared%20with%20me",
        &bearer,
        b"",
    )
    .await;

    assert_eq!(root, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(shelf, StatusCode::CONFLICT);
});

database_test!(
    a_folder_is_made_inside_a_write_share_in_the_owners_tree,
    harness,
    {
        let (owner, owner_bearer) = member(&harness, "owner@example.com", Role::Member).await;
        let (_, guest) = member(&harness, "guest@example.com", Role::Member).await;
        harness.write(owner, "shared/a.txt", b"hello").await;
        let (status, _) = call(
            &harness,
            "POST",
            "/v1/grants",
            &owner_bearer,
            br#"{"path":"shared","email":"guest@example.com","access":"write"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        let (status, _) = call(
            &harness,
            "POST",
            "/v1/folders/Shared%20with%20me/shared/from-guest",
            &guest,
            b"",
        )
        .await;

        assert_eq!(status, StatusCode::CREATED);
        assert!(
            names(&harness, "/v1/folders/shared", &owner_bearer)
                .await
                .contains(&"from-guest".to_owned())
        );
    }
);

database_test!(
    a_read_share_does_not_let_the_guest_make_a_folder,
    harness,
    {
        let (owner, owner_bearer) = member(&harness, "sharer@example.com", Role::Member).await;
        let (_, guest) = member(&harness, "viewer@example.com", Role::Member).await;
        harness.write(owner, "shared/a.txt", b"hello").await;
        call(
            &harness,
            "POST",
            "/v1/grants",
            &owner_bearer,
            br#"{"path":"shared","email":"viewer@example.com","access":"read"}"#,
        )
        .await;

        let (status, _) = call(
            &harness,
            "POST",
            "/v1/folders/Shared%20with%20me/shared/nope",
            &guest,
            b"",
        )
        .await;

        assert_eq!(status, StatusCode::FORBIDDEN);
    }
);
