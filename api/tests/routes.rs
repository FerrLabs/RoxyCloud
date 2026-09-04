mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use roxycloud_api::build_router;
use roxycloud_core::role::Role;
use tower::ServiceExt;

use common::Harness;

async fn send(harness: &Harness, request: Request<Body>) -> axum::response::Response {
    build_router(harness.state.clone(), &[], None)
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

database_test!(the_trash_lists_what_the_caller_deleted, harness, {
    let member = harness.account("trashlist@example.com", Role::Member).await;
    harness.write(member.id, "photos/x.txt", b"deep").await;
    let photos = harness.resolve(member.id, "photos").await;
    harness.trash(&photos).await;
    let token = harness.state.sessions.issue(member.id).expect("a token");

    let (status, body) = call(
        &harness,
        authorised("GET", "/v1/trash", &token, Body::empty()),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    let listed = String::from_utf8_lossy(&body);
    assert!(listed.contains(r#""name":"photos""#), "{listed}");
    assert!(
        !listed.contains(r#""name":"x.txt""#),
        "the child came along with the delete, the user did not choose it: {listed}"
    );
});

database_test!(a_restore_answers_with_the_node, harness, {
    let member = harness.account("restore@example.com", Role::Member).await;
    let node = harness.write(member.id, "notes.md", b"back again").await;
    harness.trash(&node).await;
    let token = harness.state.sessions.issue(member.id).expect("a token");

    let (status, body) = call(
        &harness,
        authorised(
            "POST",
            &format!("/v1/trash/{}/restore", node.id),
            &token,
            Body::empty(),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains(r#""name":"notes.md""#));
    let root = harness.root(member.id).await;
    assert_eq!(harness.children(&root).await, ["notes.md"]);
});

database_test!(a_purge_answers_no_content, harness, {
    let member = harness.account("purge@example.com", Role::Member).await;
    let node = harness.write(member.id, "gone.txt", b"for good").await;
    harness.trash(&node).await;
    let token = harness.state.sessions.issue(member.id).expect("a token");

    let (status, _) = call(
        &harness,
        authorised(
            "DELETE",
            &format!("/v1/trash/{}", node.id),
            &token,
            Body::empty(),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(harness.trashed(member.id).await.is_empty());
});

database_test!(a_reader_may_not_restore, harness, {
    let reader = harness.account("noretore@example.com", Role::Reader).await;
    let node = harness
        .write(reader.id, "a.txt", b"stays in the trash")
        .await;
    harness.trash(&node).await;
    let token = harness.state.sessions.issue(reader.id).expect("a token");

    let (status, _) = call(
        &harness,
        authorised(
            "POST",
            &format!("/v1/trash/{}/restore", node.id),
            &token,
            Body::empty(),
        ),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(harness.trashed(reader.id).await, ["a.txt"]);
});

database_test!(
    restoring_something_that_was_never_deleted_is_not_found,
    harness,
    {
        let member = harness.account("nothere@example.com", Role::Member).await;
        let node = harness.write(member.id, "live.txt", b"still here").await;
        let token = harness.state.sessions.issue(member.id).expect("a token");

        let (status, _) = call(
            &harness,
            authorised(
                "POST",
                &format!("/v1/trash/{}/restore", node.id),
                &token,
                Body::empty(),
            ),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }
);

fn web_root(harness: &Harness) -> std::path::PathBuf {
    let root = harness.blob_root.join("web");
    std::fs::create_dir_all(&root).expect("a web root");
    std::fs::write(root.join("index.html"), "<title>RoxyCloud</title>").expect("an entry point");
    std::fs::write(root.join("main.js"), "console.log('bundle')").expect("a bundle");
    root
}

async fn from_the_web_root(harness: &Harness, path: &str) -> (StatusCode, String) {
    let response = build_router(harness.state.clone(), &[], Some(&web_root(harness)))
        .oneshot(
            Request::builder()
                .uri(path)
                .body(Body::empty())
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

database_test!(the_web_app_is_served_when_a_root_is_configured, harness, {
    let (status, body) = from_the_web_root(&harness, "/").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("RoxyCloud"), "{body}");

    let (status, body) = from_the_web_root(&harness, "/main.js").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("bundle"), "{body}");
});

database_test!(an_unknown_api_path_is_still_not_found, harness, {
    for path in ["/v1/nonsense", "/v1", "/v1/"] {
        let (status, body) = from_the_web_root(&harness, path).await;

        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "the app must not answer for {path}, or a typo in a client reads as a page: {body}"
        );
        assert!(!body.contains("RoxyCloud"), "{path}: {body}");
    }
});

database_test!(
    the_entry_point_is_revalidated_but_the_bundle_is_not,
    harness,
    {
        let root = web_root(&harness);
        let cache_control = async |path: &str| {
            build_router(harness.state.clone(), &[], Some(&root))
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .body(Body::empty())
                        .expect("a well formed request"),
                )
                .await
                .expect("the router answers")
                .headers()
                .get(header::CACHE_CONTROL)
                .map(|value| value.to_str().expect("printable").to_owned())
        };

        assert_eq!(
            cache_control("/").await.as_deref(),
            Some("no-cache"),
            "a cached entry point outlives an upgrade and asks for a bundle that is gone"
        );
        assert_eq!(
            cache_control("/main.js").await,
            None,
            "the bundle is named after its contents, so it never goes stale"
        );
    }
);

database_test!(the_api_still_answers_with_the_app_alongside_it, harness, {
    let (status, body) = from_the_web_root(&harness, "/health").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("ok"), "{body}");

    let (status, _) = from_the_web_root(&harness, "/v1/folders").await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a route that exists still enforces its own rules"
    );
});

database_test!(a_path_outside_the_web_root_is_refused, harness, {
    for climb in ["/../Cargo.toml", "/%2e%2e/Cargo.toml", "/..%2fCargo.toml"] {
        let (status, body) = from_the_web_root(&harness, climb).await;
        assert!(
            !body.contains("[package]"),
            "{climb} escaped the web root: {body}"
        );
        assert!(
            status == StatusCode::NOT_FOUND || status == StatusCode::OK,
            "{climb} answered {status}"
        );
    }
});

database_test!(an_uploaded_page_never_comes_back_as_one, harness, {
    let member = harness.account("stored@example.com", Role::Member).await;
    let token = harness.state.sessions.issue(member.id).expect("a token");
    let page = "<script>fetch('/v1/auth/me').then(console.log)</script>";

    let (created, _) = call(
        &harness,
        authorised("PUT", "/v1/files/evil.html", &token, Body::from(page)),
    )
    .await;
    assert_eq!(created, StatusCode::CREATED);

    let response = send(
        &harness,
        authorised("GET", "/v1/files/evil.html", &token, Body::empty()),
    )
    .await;

    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    assert_eq!(
        headers
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap()),
        Some("application/octet-stream"),
        "the app and these bytes share an origin, so the type is never taken from the name"
    );
    assert_eq!(
        headers
            .get(header::CONTENT_DISPOSITION)
            .map(|v| v.to_str().unwrap()),
        Some("attachment"),
        "a browser that reaches this must save it, not render it"
    );
    assert_eq!(
        headers
            .get(header::X_CONTENT_TYPE_OPTIONS)
            .map(|v| v.to_str().unwrap()),
        Some("nosniff"),
        "and must not talk itself into rendering it either"
    );
});

database_test!(an_app_password_is_minted_listed_and_revoked, harness, {
    let member = harness.account("creds@example.com", Role::Member).await;
    let token = harness.state.sessions.issue(member.id).expect("a token");

    let (status, body) = call(
        &harness,
        Request::builder()
            .method("POST")
            .uri("/v1/app-passwords")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"name":"rclone"}"#))
            .expect("a well formed request"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let minted = String::from_utf8_lossy(&body);
    assert!(minted.contains("\"secret\""), "{minted}");

    let id = minted
        .split("\"id\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("an id in the response")
        .to_owned();

    let (status, listed) = call(
        &harness,
        authorised("GET", "/v1/app-passwords", &token, Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let listed = String::from_utf8_lossy(&listed);
    assert!(listed.contains("rclone"), "{listed}");
    assert!(
        !listed.contains("\"secret\""),
        "the secret is shown once, at the moment it is minted: {listed}"
    );

    let (status, _) = call(
        &harness,
        authorised(
            "DELETE",
            &format!("/v1/app-passwords/{id}"),
            &token,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
});

database_test!(
    minting_a_credential_needs_a_session_not_a_credential,
    harness,
    {
        let (status, _) = call(
            &harness,
            Request::builder()
                .method("POST")
                .uri("/v1/app-passwords")
                .header(
                    header::AUTHORIZATION,
                    "Basic ZGF2QGV4YW1wbGUuY29tOnNlY3JldA==",
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"name":"nice try"}"#))
                .expect("a well formed request"),
        )
        .await;

        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "account management is the one place a WebDAV credential must not reach"
        );
    }
);

database_test!(
    a_session_that_outlived_its_account_mints_nothing,
    harness,
    {
        let member = harness.account("stale@example.com", Role::Member).await;
        let token = harness.state.sessions.issue(member.id).expect("a token");
        sqlx::query("UPDATE users SET disabled_at = now() WHERE id = $1")
            .bind(member.id)
            .execute(&harness.state.db)
            .await
            .expect("disabling the account");

        let (status, _) = call(
            &harness,
            Request::builder()
                .method("POST")
                .uri("/v1/app-passwords")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"name":"after the fact"}"#))
                .expect("a well formed request"),
        )
        .await;

        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "the credential would outlive the session, so the session has to be checked here"
        );
    }
);
