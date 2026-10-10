mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use stashden_api::build_router;
use stashden_core::role::Role;
use tower::ServiceExt;
use uuid::Uuid;

use common::Harness;

const OVERSIZED: &[u8] = &[7; 400];

async fn member(harness: &Harness, email: &str, quota: i64) -> (Uuid, String) {
    let user = harness.account(email, Role::Member).await;
    harness.root(user.id).await;
    harness.set_quota(user.id, quota).await;
    let token = harness.state.sessions.issue(user.id).expect("a token");
    (user.id, format!("Bearer {token}"))
}

async fn put(
    harness: &Harness,
    uri: &str,
    authorization: &str,
    contents: &[u8],
    declared: bool,
) -> StatusCode {
    let mut request = Request::builder()
        .method("PUT")
        .uri(uri)
        .header(header::AUTHORIZATION, authorization);
    if declared {
        request = request.header(header::CONTENT_LENGTH, contents.len());
    }
    build_router(harness.state.clone(), &[], None)
        .oneshot(
            request
                .body(Body::from(contents.to_vec()))
                .expect("a well formed request"),
        )
        .await
        .expect("the router answers")
        .status()
}

async fn never_stored(harness: &Harness, contents: &[u8]) -> bool {
    let hash = blake3::hash(contents).into();
    harness.blob(hash).await.is_none() && !harness.blob_file_exists(hash).await
}

database_test!(a_declared_size_past_the_quota_is_refused_unread, harness, {
    let (_, bearer) = member(&harness, "declared@example.com", 100).await;

    let status = put(&harness, "/v1/files/big.bin", &bearer, OVERSIZED, true).await;

    assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE);
    assert!(
        never_stored(&harness, OVERSIZED).await,
        "a member at their quota must not be able to fill the disk"
    );
});

database_test!(an_undeclared_body_is_cut_off_at_the_quota, harness, {
    let (_, bearer) = member(&harness, "streamed@example.com", 100).await;

    let status = put(&harness, "/v1/files/big.bin", &bearer, OVERSIZED, false).await;

    assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE);
    assert!(never_stored(&harness, OVERSIZED).await);
});

database_test!(a_body_that_lies_about_its_size_is_cut_off, harness, {
    let (_, bearer) = member(&harness, "liar@example.com", 100).await;
    let request = Request::builder()
        .method("PUT")
        .uri("/v1/files/big.bin")
        .header(header::AUTHORIZATION, &bearer)
        .header(header::CONTENT_LENGTH, 10)
        .body(Body::from(OVERSIZED.to_vec()))
        .expect("a well formed request");

    let status = build_router(harness.state.clone(), &[], None)
        .oneshot(request)
        .await
        .expect("the router answers")
        .status();

    assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE);
    assert!(never_stored(&harness, OVERSIZED).await);
});

database_test!(an_upload_that_exactly_fills_the_quota_lands, harness, {
    let (owner, bearer) = member(&harness, "exact@example.com", 400).await;

    let status = put(&harness, "/v1/files/big.bin", &bearer, OVERSIZED, false).await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(harness.used_bytes(owner).await, 400);
});

database_test!(the_room_left_counts_what_is_already_stored, harness, {
    let (owner, bearer) = member(&harness, "nearly@example.com", 450).await;
    harness.write(owner, "kept.bin", &[1; 100]).await;

    let status = put(&harness, "/v1/files/big.bin", &bearer, OVERSIZED, true).await;

    assert_eq!(status, StatusCode::INSUFFICIENT_STORAGE);
    assert!(never_stored(&harness, OVERSIZED).await);
});

database_test!(
    a_read_only_share_is_refused_before_the_body_is_read,
    harness,
    {
        let (owner, owner_bearer) = member(&harness, "sharer@example.com", 10_000).await;
        let (_, guest) = member(&harness, "reader@example.com", 10_000).await;
        harness.write(owner, "photos/beach.jpg", b"sand").await;
        let granted = build_router(harness.state.clone(), &[], None)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/grants")
                    .header(header::AUTHORIZATION, &owner_bearer)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"path":"photos","email":"reader@example.com","access":"read"}"#,
                    ))
                    .expect("a well formed request"),
            )
            .await
            .expect("the router answers")
            .status();
        assert_eq!(granted, StatusCode::CREATED);

        let status = put(
            &harness,
            "/v1/files/Shared%20with%20me/photos/big.bin",
            &guest,
            OVERSIZED,
            true,
        )
        .await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(never_stored(&harness, OVERSIZED).await);
    }
);

database_test!(an_overwrite_can_use_the_room_its_history_frees, harness, {
    let (owner, bearer) = member(&harness, "history@example.com", 500).await;
    harness.write(owner, "a.bin", &[1; 200]).await;
    harness.write(owner, "a.bin", &[2; 200]).await;
    assert_eq!(harness.used_bytes(owner).await, 400);

    let status = put(&harness, "/v1/files/a.bin", &bearer, OVERSIZED, true).await;

    assert_eq!(
        status,
        StatusCode::CREATED,
        "a save that drops old versions to fit was accepted before the early check existed"
    );
    assert_eq!(harness.used_bytes(owner).await, 400);
});
