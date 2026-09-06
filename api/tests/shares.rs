mod common;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use roxycloud_api::build_router;
use roxycloud_core::role::Role;
use tower::ServiceExt;
use uuid::Uuid;

use common::Harness;

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Answer {
    fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    }
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

async fn anonymous(harness: &Harness, path: &str) -> Answer {
    call(harness, "GET", path, &[], "").await
}

async fn with_password(harness: &Harness, path: &str, password: &str) -> Answer {
    call(harness, "GET", path, &[("x-share-password", password)], "").await
}

async fn as_account(
    harness: &Harness,
    method: &str,
    path: &str,
    bearer: &str,
    body: &str,
) -> Answer {
    call(
        harness,
        method,
        path,
        &[(header::AUTHORIZATION.as_str(), &format!("Bearer {bearer}"))],
        body,
    )
    .await
}

async fn session(harness: &Harness, email: &str, role: Role) -> (Uuid, String) {
    let user = harness.account(email, role).await;
    let token = harness.state.sessions.issue(user.id).expect("a token");
    (user.id, token)
}

fn field<'a>(body: &'a str, name: &str) -> &'a str {
    body.split(&format!("\"{name}\":\""))
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no {name} in {body}"))
}

async fn link_to(harness: &Harness, bearer: &str, path: &str) -> String {
    let minted = as_account(
        harness,
        "POST",
        "/v1/shares",
        bearer,
        &format!(r#"{{"path":"{path}"}}"#),
    )
    .await;
    assert_eq!(minted.status, StatusCode::CREATED, "{}", minted.body);
    field(&minted.body, "token").to_owned()
}

database_test!(
    a_link_serves_its_file_to_a_visitor_with_no_account,
    harness,
    {
        let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
        harness
            .write(id, "notes/hello.txt", b"the shared bytes")
            .await;

        let token = link_to(&harness, &bearer, "notes/hello.txt").await;
        let served = anonymous(&harness, &format!("/v1/public/{token}/content")).await;

        assert_eq!(served.status, StatusCode::OK);
        assert_eq!(served.body, "the shared bytes");
    }
);

database_test!(shared_bytes_are_never_rendered_on_this_origin, harness, {
    let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
    harness
        .write(id, "page.html", b"<script>alert(1)</script>")
        .await;
    let token = link_to(&harness, &bearer, "page.html").await;

    let served = anonymous(&harness, &format!("/v1/public/{token}/content")).await;

    assert_eq!(served.header("content-type"), "application/octet-stream");
    assert_eq!(served.header("content-disposition"), "attachment");
    assert_eq!(
        served.header("x-content-type-options"),
        "nosniff",
        "a link serves a file to a browser carrying no credentials, which is the one case where \
         stored bytes are rendered by a navigation rather than fetched by the app"
    );
});

database_test!(a_revoked_link_stops_working_at_once, harness, {
    let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
    harness.write(id, "secret.txt", b"private").await;
    let minted = as_account(
        &harness,
        "POST",
        "/v1/shares",
        &bearer,
        r#"{"path":"secret.txt"}"#,
    )
    .await;
    let token = field(&minted.body, "token").to_owned();
    let share = field(&minted.body, "id").to_owned();

    let before = anonymous(&harness, &format!("/v1/public/{token}/content")).await;
    let revoked = as_account(
        &harness,
        "DELETE",
        &format!("/v1/shares/{share}"),
        &bearer,
        "",
    )
    .await;
    let after = anonymous(&harness, &format!("/v1/public/{token}/content")).await;

    assert_eq!(before.status, StatusCode::OK);
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    assert_eq!(after.status, StatusCode::NOT_FOUND);
});

database_test!(an_expired_link_stops_working, harness, {
    let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
    harness.write(id, "seasonal.txt", b"gone by monday").await;
    let token = link_to(&harness, &bearer, "seasonal.txt").await;

    let before = anonymous(&harness, &format!("/v1/public/{token}/content")).await;
    harness.expire_shares().await;
    let after = anonymous(&harness, &format!("/v1/public/{token}/content")).await;

    assert_eq!(before.status, StatusCode::OK);
    assert_eq!(after.status, StatusCode::NOT_FOUND);
});

database_test!(a_token_for_one_node_can_never_read_another, harness, {
    let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
    harness
        .write(id, "public/shared.txt", b"fine to read")
        .await;
    harness.write(id, "private/payslip.txt", b"not fine").await;
    let token = link_to(&harness, &bearer, "public").await;

    let sibling = anonymous(
        &harness,
        &format!("/v1/public/{token}/content/private/payslip.txt"),
    )
    .await;
    let climbing = anonymous(
        &harness,
        &format!("/v1/public/{token}/content/../private/payslip.txt"),
    )
    .await;
    let inside = anonymous(&harness, &format!("/v1/public/{token}/content/shared.txt")).await;

    assert_eq!(
        sibling.status,
        StatusCode::NOT_FOUND,
        "a path is walked downward from the shared node, so a name outside it resolves to nothing"
    );
    assert_ne!(climbing.status, StatusCode::OK, "{}", climbing.body);
    assert_eq!(inside.body, "fine to read");
});

database_test!(
    a_token_does_not_serve_what_a_different_token_was_minted_for,
    harness,
    {
        let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
        harness.write(id, "one.txt", b"one").await;
        harness.write(id, "two.txt", b"two").await;
        let one = link_to(&harness, &bearer, "one.txt").await;
        let two = link_to(&harness, &bearer, "two.txt").await;

        let first = anonymous(&harness, &format!("/v1/public/{one}/content")).await;
        let second = anonymous(&harness, &format!("/v1/public/{two}/content")).await;

        assert_eq!(first.body, "one");
        assert_eq!(second.body, "two");
    }
);

database_test!(a_password_protected_link_needs_the_password, harness, {
    let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
    harness.write(id, "guarded.txt", b"behind a password").await;
    let minted = as_account(
        &harness,
        "POST",
        "/v1/shares",
        &bearer,
        r#"{"path":"guarded.txt","password":"twelve-characters-at-least"}"#,
    )
    .await;
    assert_eq!(minted.status, StatusCode::CREATED, "{}", minted.body);
    let token = field(&minted.body, "token").to_owned();
    let path = format!("/v1/public/{token}/content");

    let bare = anonymous(&harness, &path).await;
    let wrong = with_password(&harness, &path, "not-the-password").await;
    let right = with_password(&harness, &path, "twelve-characters-at-least").await;

    assert_eq!(bare.status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(right.status, StatusCode::OK);
    assert_eq!(right.body, "behind a password");
});

database_test!(a_link_follows_its_node_into_the_trash_and_back, harness, {
    let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
    let node = harness.write(id, "draft.txt", b"still here").await;
    let token = link_to(&harness, &bearer, "draft.txt").await;
    let path = format!("/v1/public/{token}/content");

    harness.trash(&node).await;
    let trashed = anonymous(&harness, &path).await;

    harness.restore(id, node.id).await;
    let restored = anonymous(&harness, &path).await;

    assert_eq!(
        trashed.status,
        StatusCode::NOT_FOUND,
        "deleting a file has to take down the links to it"
    );
    assert_eq!(restored.status, StatusCode::OK);
});

database_test!(purging_the_node_takes_the_link_with_it, harness, {
    let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
    let node = harness.write(id, "gone.txt", b"soon").await;
    let token = link_to(&harness, &bearer, "gone.txt").await;

    harness.trash(&node).await;
    harness.purge(id, node.id).await;

    let after = anonymous(&harness, &format!("/v1/public/{token}/content")).await;
    assert_eq!(after.status, StatusCode::NOT_FOUND);
    assert_eq!(
        harness.share_rows().await,
        0,
        "a purged node leaves no row behind to resolve"
    );
});

database_test!(
    a_link_minted_by_an_account_since_disabled_stops_working,
    harness,
    {
        let (_, admin) = session(&harness, "admin@example.com", Role::Admin).await;
        let (id, bearer) = session(&harness, "leaving@example.com", Role::Member).await;
        harness.write(id, "theirs.txt", b"was public").await;
        let token = link_to(&harness, &bearer, "theirs.txt").await;

        as_account(
            &harness,
            "POST",
            &format!("/v1/users/{id}/disable"),
            &admin,
            "",
        )
        .await;

        let after = anonymous(&harness, &format!("/v1/public/{token}/content")).await;
        assert_eq!(
            after.status,
            StatusCode::NOT_FOUND,
            "disabling an account has to take its links down with its sessions"
        );
    }
);

database_test!(a_directory_link_lists_what_is_under_it, harness, {
    let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
    harness.write(id, "album/one.txt", b"one").await;
    harness.write(id, "album/deeper/two.txt", b"two").await;
    harness
        .write(id, "elsewhere.txt", b"not in the album")
        .await;
    let token = link_to(&harness, &bearer, "album").await;

    let root = anonymous(&harness, &format!("/v1/public/{token}")).await;
    let deeper = anonymous(&harness, &format!("/v1/public/{token}/entries/deeper")).await;

    assert_eq!(root.status, StatusCode::OK);
    assert!(root.body.contains("one.txt"), "{}", root.body);
    assert!(root.body.contains("deeper"), "{}", root.body);
    assert!(!root.body.contains("elsewhere.txt"), "{}", root.body);
    assert!(deeper.body.contains("two.txt"), "{}", deeper.body);
});

database_test!(
    a_visitor_is_told_about_the_files_and_nothing_about_the_account,
    harness,
    {
        let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
        let node = harness.write(id, "album/one.txt", b"one").await;
        let token = link_to(&harness, &bearer, "album").await;

        let root = anonymous(&harness, &format!("/v1/public/{token}")).await;

        assert!(!root.body.contains(&id.to_string()), "{}", root.body);
        assert!(
            !root.body.contains(&node.id.to_string()),
            "a listing that names node ids hands an anonymous visitor identifiers for the rest of \
             the API: {}",
            root.body
        );
    }
);

database_test!(a_reader_may_not_publish_a_link, harness, {
    let (id, bearer) = session(&harness, "reader@example.com", Role::Reader).await;
    harness.write(id, "theirs.txt", b"readable").await;

    let minted = as_account(
        &harness,
        "POST",
        "/v1/shares",
        &bearer,
        r#"{"path":"theirs.txt"}"#,
    )
    .await;

    assert_eq!(
        minted.status,
        StatusCode::FORBIDDEN,
        "publishing bytes to anyone holding a URL is not a read"
    );
});

database_test!(
    only_the_account_that_minted_a_link_can_revoke_it,
    harness,
    {
        let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
        let (_, other) = session(&harness, "other@example.com", Role::Member).await;
        harness.write(id, "theirs.txt", b"private").await;
        let minted = as_account(
            &harness,
            "POST",
            "/v1/shares",
            &bearer,
            r#"{"path":"theirs.txt"}"#,
        )
        .await;
        let token = field(&minted.body, "token").to_owned();
        let share = field(&minted.body, "id").to_owned();

        let refused = as_account(
            &harness,
            "DELETE",
            &format!("/v1/shares/{share}"),
            &other,
            "",
        )
        .await;
        let still = anonymous(&harness, &format!("/v1/public/{token}/content")).await;

        assert_eq!(refused.status, StatusCode::NOT_FOUND);
        assert_eq!(still.status, StatusCode::OK);
    }
);

database_test!(
    the_listing_shows_the_owner_their_links_without_the_tokens,
    harness,
    {
        let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
        harness.write(id, "shared.txt", b"bytes").await;
        let token = link_to(&harness, &bearer, "shared.txt").await;

        let listed = as_account(&harness, "GET", "/v1/shares", &bearer, "").await;

        assert!(listed.body.contains("shared.txt"), "{}", listed.body);
        assert!(
            !listed.body.contains(&token),
            "a token the server can hand back twice would not need to be minted once: {}",
            listed.body
        );
    }
);

database_test!(a_link_that_would_be_born_useless_is_refused, harness, {
    let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
    harness.write(id, "theirs.txt", b"bytes").await;

    let past = as_account(
        &harness,
        "POST",
        "/v1/shares",
        &bearer,
        r#"{"path":"theirs.txt","expires_at":"2020-01-01T00:00:00Z"}"#,
    )
    .await;
    let weak = as_account(
        &harness,
        "POST",
        "/v1/shares",
        &bearer,
        r#"{"path":"theirs.txt","password":"short"}"#,
    )
    .await;
    let root = as_account(&harness, "POST", "/v1/shares", &bearer, r#"{"path":""}"#).await;

    assert_eq!(past.status, StatusCode::BAD_REQUEST);
    assert_eq!(weak.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        root.status,
        StatusCode::BAD_REQUEST,
        "sharing the root would publish the whole account: {}",
        root.body
    );
});

database_test!(a_token_nobody_minted_is_not_found, harness, {
    let answer = anonymous(&harness, "/v1/public/deadbeef/content").await;
    assert_eq!(answer.status, StatusCode::NOT_FOUND);
});
