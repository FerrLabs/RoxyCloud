mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use roxycloud_api::build_router;
use roxycloud_core::role::Role;
use tower::ServiceExt;

use common::Harness;

async fn send(harness: &Harness, request: Request<Body>) -> axum::response::Response {
    build_router(harness.state.clone(), &[])
        .oneshot(request)
        .await
        .expect("the router answers")
}

async fn call(harness: &Harness, request: Request<Body>) -> (StatusCode, Vec<u8>) {
    let response = send(harness, request).await;
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("reading the body")
        .to_bytes()
        .to_vec();
    (status, body)
}

fn authorised(method: &str, path: &str, token: &str, body: Body) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(body)
        .expect("a well formed request")
}

fn move_request(token: &str, from: &str, to: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/move")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(r#"{{"from":"{from}","to":"{to}"}}"#)))
        .expect("a well formed request")
}

database_test!(a_first_upload_answers_created, harness, {
    let owner = harness.account("first@example.com", Role::Member).await;
    let token = harness.state.sessions.issue(owner.id).expect("a token");

    let (status, _) = call(
        &harness,
        authorised(
            "PUT",
            "/v1/files/notes.md",
            &token,
            Body::from("first bytes"),
        ),
    )
    .await;

    assert_eq!(
        status,
        StatusCode::CREATED,
        "an account's very first write creates its root on the way through"
    );
});

database_test!(a_reader_may_read, harness, {
    let reader = harness.account("reader@example.com", Role::Reader).await;
    harness.write(reader.id, "a.txt", b"already there").await;
    let token = harness.state.sessions.issue(reader.id).expect("a token");

    let (status, body) = call(
        &harness,
        authorised("GET", "/v1/folders", &token, Body::empty()),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        String::from_utf8_lossy(&body).contains("a.txt"),
        "the listing came back"
    );
});

database_test!(a_reader_may_not_upload, harness, {
    let reader = harness.account("noupload@example.com", Role::Reader).await;
    let token = harness.state.sessions.issue(reader.id).expect("a token");

    let (status, _) = call(
        &harness,
        authorised("PUT", "/v1/files/a.txt", &token, Body::from("nope")),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
});

database_test!(a_reader_may_not_delete, harness, {
    let reader = harness.account("nodelete@example.com", Role::Reader).await;
    harness
        .write(reader.id, "a.txt", b"still here afterwards")
        .await;
    let token = harness.state.sessions.issue(reader.id).expect("a token");

    let (status, _) = call(
        &harness,
        authorised("DELETE", "/v1/files/a.txt", &token, Body::empty()),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    let root = harness.root(reader.id).await;
    assert_eq!(
        harness.children(&root).await,
        ["a.txt"],
        "nothing was trashed"
    );
});

database_test!(a_member_may_write_and_delete, harness, {
    let member = harness.account("member@example.com", Role::Member).await;
    let token = harness.state.sessions.issue(member.id).expect("a token");

    let (created, _) = call(
        &harness,
        authorised("PUT", "/v1/files/a.txt", &token, Body::from("mine")),
    )
    .await;
    let (deleted, _) = call(
        &harness,
        authorised("DELETE", "/v1/files/a.txt", &token, Body::empty()),
    )
    .await;

    assert_eq!(created, StatusCode::CREATED);
    assert_eq!(deleted, StatusCode::NO_CONTENT);
});

database_test!(
    a_disabled_account_is_refused_even_with_a_valid_token,
    harness,
    {
        let member = harness.account("disabled@example.com", Role::Member).await;
        let token = harness.state.sessions.issue(member.id).expect("a token");
        sqlx::query("UPDATE users SET disabled_at = now() WHERE id = $1")
            .bind(member.id)
            .execute(&harness.state.db)
            .await
            .expect("disabling the account");

        let (status, _) = call(
            &harness,
            authorised(
                "PUT",
                "/v1/files/a.txt",
                &token,
                Body::from("after the fact"),
            ),
        )
        .await;

        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "a token outlives the account it names"
        );
    }
);

database_test!(a_request_without_a_token_is_refused, harness, {
    let (status, _) = call(
        &harness,
        Request::builder()
            .method("GET")
            .uri("/v1/folders")
            .body(Body::empty())
            .expect("a well formed request"),
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
});

database_test!(a_path_that_climbs_out_of_the_root_is_refused, harness, {
    let member = harness.account("traversal@example.com", Role::Member).await;
    let token = harness.state.sessions.issue(member.id).expect("a token");

    let (status, _) = call(
        &harness,
        authorised(
            "PUT",
            "/v1/files/photos/../../etc/passwd",
            &token,
            Body::from("nope"),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
});

database_test!(a_move_answers_with_the_moved_node, harness, {
    let member = harness.account("mover@example.com", Role::Member).await;
    harness.write(member.id, "inbox/a.txt", b"moving out").await;
    let token = harness.state.sessions.issue(member.id).expect("a token");

    let response = send(&harness, move_request(&token, "/inbox/a.txt", "/b.txt")).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers().contains_key(header::ETAG),
        "a client that caches on the header should not have to GET the node to learn its tag"
    );
    let body = response
        .into_body()
        .collect()
        .await
        .expect("reading the body")
        .to_bytes();
    assert!(
        String::from_utf8_lossy(&body).contains(r#""name":"b.txt""#),
        "the response describes the node at its new place"
    );
    let root = harness.root(member.id).await;
    assert_eq!(harness.children(&root).await, ["b.txt", "inbox"]);
});

database_test!(a_reader_may_not_move, harness, {
    let reader = harness.account("nomove@example.com", Role::Reader).await;
    harness.write(reader.id, "a.txt", b"stays put").await;
    let token = harness.state.sessions.issue(reader.id).expect("a token");

    let (status, _) = call(&harness, move_request(&token, "/a.txt", "/b.txt")).await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    let root = harness.root(reader.id).await;
    assert_eq!(harness.children(&root).await, ["a.txt"]);
});

database_test!(a_move_onto_an_occupied_name_answers_conflict, harness, {
    let member = harness.account("clash@example.com", Role::Member).await;
    harness.write(member.id, "a.txt", b"the occupant").await;
    harness.write(member.id, "b.txt", b"the arrival").await;
    let token = harness.state.sessions.issue(member.id).expect("a token");

    let (status, _) = call(&harness, move_request(&token, "/b.txt", "/a.txt")).await;

    assert_eq!(status, StatusCode::CONFLICT);
});

database_test!(
    a_move_into_an_unknown_directory_answers_not_found,
    harness,
    {
        let member = harness.account("nowhere@example.com", Role::Member).await;
        harness
            .write(member.id, "a.txt", b"has nowhere to go")
            .await;
        let token = harness.state.sessions.issue(member.id).expect("a token");

        let (status, _) = call(&harness, move_request(&token, "/a.txt", "/absent/a.txt")).await;

        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "a move does not create the directories it lands in"
        );
    }
);
