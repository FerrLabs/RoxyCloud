mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use roxycloud_api::build_router;
use roxycloud_core::role::Role;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

use common::Harness;

struct Answer {
    status: StatusCode,
    body: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|_| panic!("json in {}", self.body))
    }

    fn names(&self) -> Vec<String> {
        self.json()
            .as_array()
            .unwrap_or_else(|| panic!("a listing in {}", self.body))
            .iter()
            .map(|entry| entry["name"].as_str().unwrap_or_default().to_owned())
            .collect()
    }
}

fn encoded(path: &str) -> String {
    path.replace(' ', "%20")
}

async fn call(harness: &Harness, method: &str, path: &str, bearer: &str, body: Vec<u8>) -> Answer {
    let response = build_router(harness.state.clone(), &[], None)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(encoded(path))
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
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

async fn get(harness: &Harness, path: &str, bearer: &str) -> Answer {
    call(harness, "GET", path, bearer, Vec::new()).await
}

async fn put(harness: &Harness, path: &str, bearer: &str, contents: &[u8]) -> Answer {
    call(harness, "PUT", path, bearer, contents.to_vec()).await
}

async fn delete(harness: &Harness, path: &str, bearer: &str) -> Answer {
    call(harness, "DELETE", path, bearer, Vec::new()).await
}

async fn post(harness: &Harness, path: &str, bearer: &str, body: &str) -> Answer {
    call(harness, "POST", path, bearer, body.as_bytes().to_vec()).await
}

async fn move_to(harness: &Harness, bearer: &str, from: &str, to: &str) -> Answer {
    post(
        harness,
        "/v1/move",
        bearer,
        &format!(r#"{{"from":"{from}","to":"{to}"}}"#),
    )
    .await
}

async fn session(harness: &Harness, email: &str, role: Role) -> (Uuid, String) {
    let user = harness.account(email, role).await;
    let token = harness.state.sessions.issue(user.id).expect("a token");
    (user.id, token)
}

async fn grant(harness: &Harness, bearer: &str, path: &str, email: &str, access: &str) -> Answer {
    post(
        harness,
        "/v1/grants",
        bearer,
        &format!(r#"{{"path":"{path}","email":"{email}","access":"{access}"}}"#),
    )
    .await
}

async fn granted(harness: &Harness, bearer: &str, path: &str, email: &str, access: &str) -> String {
    let given = grant(harness, bearer, path, email, access).await;
    assert_eq!(given.status, StatusCode::CREATED, "{}", given.body);
    given.json()["id"].as_str().expect("a grant id").to_owned()
}

database_test!(
    a_shared_folder_appears_for_the_account_it_was_shared_with,
    harness,
    {
        let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
        let (_, guest) = session(&harness, "guest@example.com", Role::Member).await;
        harness
            .write(owner, "family/photos/beach.jpg", b"sand")
            .await;

        granted(
            &harness,
            &owner_bearer,
            "family/photos",
            "guest@example.com",
            "read",
        )
        .await;

        assert_eq!(
            get(&harness, "/v1/folders", &guest).await.names(),
            ["Shared with me"]
        );
        assert_eq!(
            get(&harness, "/v1/folders/Shared with me", &guest)
                .await
                .names(),
            ["photos"]
        );
        let read = get(
            &harness,
            "/v1/files/Shared with me/photos/beach.jpg",
            &guest,
        )
        .await;
        assert_eq!(read.status, StatusCode::OK);
        assert_eq!(read.body, "sand");
    }
);

database_test!(
    nothing_beside_the_shared_folder_resolves_through_it,
    harness,
    {
        let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
        let (_, guest) = session(&harness, "guest@example.com", Role::Member).await;
        harness
            .write(owner, "family/photos/beach.jpg", b"sand")
            .await;
        harness.write(owner, "family/diary.txt", b"private").await;

        granted(
            &harness,
            &owner_bearer,
            "family/photos",
            "guest@example.com",
            "read",
        )
        .await;

        for path in [
            "/v1/files/Shared with me/photos/diary.txt",
            "/v1/files/Shared with me/family/diary.txt",
            "/v1/files/family/diary.txt",
        ] {
            assert_eq!(
                get(&harness, path, &guest).await.status,
                StatusCode::NOT_FOUND,
                "{path} reached a file that was not shared"
            );
        }
    }
);

database_test!(an_account_with_nothing_shared_has_no_shelf, harness, {
    let (id, bearer) = session(&harness, "alone@example.com", Role::Member).await;
    harness.write(id, "notes.txt", b"mine").await;

    assert_eq!(
        get(&harness, "/v1/folders", &bearer).await.names(),
        ["notes.txt"]
    );
});

database_test!(a_read_grant_does_not_write, harness, {
    let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
    let (_, guest) = session(&harness, "guest@example.com", Role::Member).await;
    harness.write(owner, "photos/beach.jpg", b"sand").await;
    granted(
        &harness,
        &owner_bearer,
        "photos",
        "guest@example.com",
        "read",
    )
    .await;

    let written = put(
        &harness,
        "/v1/files/Shared with me/photos/new.txt",
        &guest,
        b"x",
    )
    .await;
    let deleted = delete(
        &harness,
        "/v1/files/Shared with me/photos/beach.jpg",
        &guest,
    )
    .await;
    let renamed = move_to(
        &harness,
        &guest,
        "Shared with me/photos/beach.jpg",
        "Shared with me/photos/dune.jpg",
    )
    .await;

    assert_eq!(written.status, StatusCode::FORBIDDEN);
    assert_eq!(deleted.status, StatusCode::FORBIDDEN);
    assert_eq!(renamed.status, StatusCode::FORBIDDEN);
    assert_eq!(
        harness
            .children(&harness.resolve(owner, "photos").await)
            .await,
        ["beach.jpg"]
    );
});

database_test!(
    a_write_grant_creates_renames_and_deletes_in_the_owners_tree,
    harness,
    {
        let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
        let (guest_id, guest) = session(&harness, "guest@example.com", Role::Member).await;
        harness.write(owner, "photos/beach.jpg", b"sand").await;
        granted(
            &harness,
            &owner_bearer,
            "photos",
            "guest@example.com",
            "write",
        )
        .await;

        let written = put(
            &harness,
            "/v1/files/Shared with me/photos/trip/new.txt",
            &guest,
            b"x",
        )
        .await;
        assert_eq!(written.status, StatusCode::CREATED, "{}", written.body);
        assert_eq!(
            harness.resolve(owner, "photos/trip/new.txt").await.owner_id,
            owner
        );

        let renamed = move_to(
            &harness,
            &guest,
            "Shared with me/photos/beach.jpg",
            "Shared with me/photos/trip/dune.jpg",
        )
        .await;
        assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.body);

        let deleted = delete(
            &harness,
            "/v1/files/Shared with me/photos/trip/new.txt",
            &guest,
        )
        .await;
        assert_eq!(deleted.status, StatusCode::NO_CONTENT);
        assert_eq!(harness.trashed(owner).await, ["new.txt"]);
        assert!(harness.trashed(guest_id).await.is_empty());
    }
);

database_test!(a_grantees_write_is_charged_to_the_owner, harness, {
    let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
    let (guest_id, guest) = session(&harness, "guest@example.com", Role::Member).await;
    harness.write(owner, "photos/beach.jpg", b"sand").await;
    harness.root(guest_id).await;
    granted(
        &harness,
        &owner_bearer,
        "photos",
        "guest@example.com",
        "write",
    )
    .await;
    let before = harness.used_bytes(owner).await;

    let written = put(
        &harness,
        "/v1/files/Shared with me/photos/big.bin",
        &guest,
        &[7; 400],
    )
    .await;

    assert_eq!(written.status, StatusCode::CREATED, "{}", written.body);
    assert_eq!(harness.used_bytes(owner).await, before + 400);
    assert_eq!(harness.used_bytes(guest_id).await, 0);
});

database_test!(a_grantee_cannot_write_past_the_owners_quota, harness, {
    let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
    let (_, guest) = session(&harness, "guest@example.com", Role::Member).await;
    harness.write(owner, "photos/beach.jpg", b"sand").await;
    granted(
        &harness,
        &owner_bearer,
        "photos",
        "guest@example.com",
        "write",
    )
    .await;
    harness.set_quota(owner, 100).await;

    let written = put(
        &harness,
        "/v1/files/Shared with me/photos/big.bin",
        &guest,
        &[7; 400],
    )
    .await;

    assert_eq!(written.status, StatusCode::INSUFFICIENT_STORAGE);
});

database_test!(
    a_resumable_upload_into_a_share_asks_the_owners_quota,
    harness,
    {
        let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
        let (_, guest) = session(&harness, "guest@example.com", Role::Member).await;
        harness.write(owner, "photos/beach.jpg", b"sand").await;
        granted(
            &harness,
            &owner_bearer,
            "photos",
            "guest@example.com",
            "write",
        )
        .await;
        harness.set_quota(owner, 100).await;

        let opened = post(
            &harness,
            "/v1/uploads",
            &guest,
            r#"{"path":"Shared with me/photos/big.bin","size":400}"#,
        )
        .await;

        assert_eq!(
            opened.status,
            StatusCode::INSUFFICIENT_STORAGE,
            "{}",
            opened.body
        );
    }
);

database_test!(a_grantee_deleting_serialises_on_the_owners_lock, harness, {
    let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
    let (_, guest) = session(&harness, "guest@example.com", Role::Member).await;
    harness.write(owner, "photos/beach.jpg", b"sand").await;
    granted(
        &harness,
        &owner_bearer,
        "photos",
        "guest@example.com",
        "write",
    )
    .await;

    let held = harness.hold_account_lock(owner).await;
    let router = build_router(harness.state.clone(), &[], None);
    let deleting = tokio::spawn(async move {
        router
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(encoded("/v1/files/Shared with me/photos/beach.jpg"))
                    .header(header::AUTHORIZATION, format!("Bearer {guest}"))
                    .body(Body::empty())
                    .expect("a well formed request"),
            )
            .await
            .expect("the router answers")
            .status()
    });

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !deleting.is_finished(),
        "a grantee's write has to wait on the owner's lock, not take its own"
    );

    held.rollback().await.expect("releasing the lock");
    assert_eq!(
        deleting.await.expect("the delete task"),
        StatusCode::NO_CONTENT
    );
});

database_test!(
    a_reader_account_does_not_write_through_a_write_grant,
    harness,
    {
        let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
        let (_, guest) = session(&harness, "reader@example.com", Role::Reader).await;
        harness.write(owner, "photos/beach.jpg", b"sand").await;
        granted(
            &harness,
            &owner_bearer,
            "photos",
            "reader@example.com",
            "write",
        )
        .await;

        let written = put(
            &harness,
            "/v1/files/Shared with me/photos/new.txt",
            &guest,
            b"x",
        )
        .await;
        let read = get(
            &harness,
            "/v1/files/Shared with me/photos/beach.jpg",
            &guest,
        )
        .await;

        assert_eq!(written.status, StatusCode::FORBIDDEN);
        assert_eq!(read.status, StatusCode::OK);
    }
);

database_test!(a_move_across_the_boundary_is_refused_both_ways, harness, {
    let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
    let (guest_id, guest) = session(&harness, "guest@example.com", Role::Member).await;
    harness.write(owner, "photos/beach.jpg", b"sand").await;
    harness.write(guest_id, "mine.txt", b"mine").await;
    granted(
        &harness,
        &owner_bearer,
        "photos",
        "guest@example.com",
        "write",
    )
    .await;

    let out = move_to(
        &harness,
        &guest,
        "Shared with me/photos/beach.jpg",
        "beach.jpg",
    )
    .await;
    let into = move_to(
        &harness,
        &guest,
        "mine.txt",
        "Shared with me/photos/mine.txt",
    )
    .await;

    assert_eq!(out.status, StatusCode::FORBIDDEN, "{}", out.body);
    assert_eq!(into.status, StatusCode::FORBIDDEN, "{}", into.body);
    assert_eq!(
        harness.resolve(owner, "photos/beach.jpg").await.owner_id,
        owner
    );
    assert_eq!(
        harness.resolve(guest_id, "mine.txt").await.owner_id,
        guest_id
    );
});

database_test!(
    the_shared_folder_itself_is_not_the_grantees_to_move_or_delete,
    harness,
    {
        let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
        let (_, guest) = session(&harness, "guest@example.com", Role::Member).await;
        harness.write(owner, "photos/beach.jpg", b"sand").await;
        granted(
            &harness,
            &owner_bearer,
            "photos",
            "guest@example.com",
            "write",
        )
        .await;

        let deleted = delete(&harness, "/v1/files/Shared with me/photos", &guest).await;
        let renamed = move_to(
            &harness,
            &guest,
            "Shared with me/photos",
            "Shared with me/pics",
        )
        .await;

        assert_eq!(deleted.status, StatusCode::FORBIDDEN);
        assert_eq!(renamed.status, StatusCode::FORBIDDEN);
        assert_eq!(harness.resolve(owner, "photos").await.owner_id, owner);
    }
);

database_test!(a_grantee_cannot_share_onward, harness, {
    let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
    let (_, guest) = session(&harness, "guest@example.com", Role::Member).await;
    harness.account("third@example.com", Role::Member).await;
    harness.write(owner, "photos/beach.jpg", b"sand").await;
    granted(
        &harness,
        &owner_bearer,
        "photos",
        "guest@example.com",
        "write",
    )
    .await;

    let regranted = grant(
        &harness,
        &guest,
        "Shared with me/photos",
        "third@example.com",
        "read",
    )
    .await;
    let published = post(
        &harness,
        "/v1/shares",
        &guest,
        r#"{"path":"Shared with me/photos/beach.jpg"}"#,
    )
    .await;

    assert_eq!(
        regranted.status,
        StatusCode::FORBIDDEN,
        "{}",
        regranted.body
    );
    assert_eq!(
        published.status,
        StatusCode::FORBIDDEN,
        "{}",
        published.body
    );
});

database_test!(a_revoked_grant_is_gone_on_the_next_request, harness, {
    let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
    let (_, guest) = session(&harness, "guest@example.com", Role::Member).await;
    harness.write(owner, "photos/beach.jpg", b"sand").await;
    let id = granted(
        &harness,
        &owner_bearer,
        "photos",
        "guest@example.com",
        "read",
    )
    .await;
    let path = "/v1/files/Shared with me/photos/beach.jpg";
    assert_eq!(get(&harness, path, &guest).await.status, StatusCode::OK);

    let revoked = delete(&harness, &format!("/v1/grants/{id}"), &owner_bearer).await;

    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    assert_eq!(
        get(&harness, path, &guest).await.status,
        StatusCode::NOT_FOUND
    );
    assert!(
        get(&harness, "/v1/folders", &guest)
            .await
            .names()
            .is_empty()
    );
});

database_test!(
    a_grantee_can_leave_a_share_without_touching_the_folder,
    harness,
    {
        let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
        let (_, guest) = session(&harness, "guest@example.com", Role::Member).await;
        harness.write(owner, "photos/beach.jpg", b"sand").await;
        let id = granted(
            &harness,
            &owner_bearer,
            "photos",
            "guest@example.com",
            "read",
        )
        .await;

        let left = delete(&harness, &format!("/v1/grants/{id}"), &guest).await;

        assert_eq!(left.status, StatusCode::NO_CONTENT);
        assert_eq!(
            harness.resolve(owner, "photos/beach.jpg").await.owner_id,
            owner
        );
    }
);

database_test!(somebody_else_cannot_withdraw_a_grant, harness, {
    let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
    let (_, stranger) = session(&harness, "stranger@example.com", Role::Member).await;
    harness.account("guest@example.com", Role::Member).await;
    harness.write(owner, "photos/beach.jpg", b"sand").await;
    let id = granted(
        &harness,
        &owner_bearer,
        "photos",
        "guest@example.com",
        "read",
    )
    .await;

    let withdrawn = delete(&harness, &format!("/v1/grants/{id}"), &stranger).await;

    assert_eq!(withdrawn.status, StatusCode::NOT_FOUND);
    assert_eq!(
        get(&harness, "/v1/grants", &owner_bearer)
            .await
            .json()
            .as_array()
            .map(Vec::len),
        Some(1)
    );
});

database_test!(
    granting_to_an_address_without_an_account_answers_the_same,
    harness,
    {
        let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
        harness.account("known@example.com", Role::Member).await;
        harness.write(owner, "photos/beach.jpg", b"sand").await;
        harness.write(owner, "music/song.ogg", b"la").await;

        let known = grant(
            &harness,
            &owner_bearer,
            "photos",
            "known@example.com",
            "read",
        )
        .await;
        let unknown = grant(
            &harness,
            &owner_bearer,
            "music",
            "nobody@example.com",
            "read",
        )
        .await;

        assert_eq!(known.status, unknown.status);
        let (known, unknown) = (known.json(), unknown.json());
        let keys = |value: &Value| {
            let mut keys: Vec<String> = value
                .as_object()
                .expect("an object")
                .keys()
                .cloned()
                .collect();
            keys.sort();
            keys
        };
        assert_eq!(keys(&known), keys(&unknown));
        assert_eq!(known["access"], unknown["access"]);
    }
);

database_test!(
    an_account_created_later_for_the_address_receives_the_share,
    harness,
    {
        let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
        harness.write(owner, "photos/beach.jpg", b"sand").await;
        granted(
            &harness,
            &owner_bearer,
            "photos",
            "later@example.com",
            "read",
        )
        .await;

        let (_, later) = session(&harness, "later@example.com", Role::Member).await;

        assert_eq!(
            get(
                &harness,
                "/v1/files/Shared with me/photos/beach.jpg",
                &later
            )
            .await
            .body,
            "sand"
        );
    }
);

database_test!(
    a_grant_inside_a_folder_already_shared_with_that_address_is_refused,
    harness,
    {
        let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
        harness.account("guest@example.com", Role::Member).await;
        harness
            .write(owner, "family/photos/beach.jpg", b"sand")
            .await;
        granted(
            &harness,
            &owner_bearer,
            "family",
            "guest@example.com",
            "read",
        )
        .await;

        let inner = grant(
            &harness,
            &owner_bearer,
            "family/photos",
            "guest@example.com",
            "write",
        )
        .await;
        let outer = grant(
            &harness,
            &owner_bearer,
            "family",
            "guest@example.com",
            "write",
        )
        .await;

        assert_eq!(inner.status, StatusCode::CONFLICT, "{}", inner.body);
        assert_eq!(outer.status, StatusCode::CONFLICT, "{}", outer.body);
    }
);

database_test!(two_shares_of_the_same_name_are_told_apart, harness, {
    let (first, first_bearer) = session(&harness, "first@example.com", Role::Member).await;
    let (second, second_bearer) = session(&harness, "second@example.com", Role::Member).await;
    let (_, guest) = session(&harness, "guest@example.com", Role::Member).await;
    harness.write(first, "Photos/a.jpg", b"first").await;
    harness.write(second, "Photos/b.jpg", b"second").await;

    granted(
        &harness,
        &first_bearer,
        "Photos",
        "guest@example.com",
        "read",
    )
    .await;
    granted(
        &harness,
        &second_bearer,
        "Photos",
        "guest@example.com",
        "read",
    )
    .await;

    assert_eq!(
        get(&harness, "/v1/folders/Shared with me", &guest)
            .await
            .names(),
        ["Photos", "Photos (2)"]
    );
    assert_eq!(
        get(
            &harness,
            "/v1/files/Shared with me/Photos (2)/b.jpg",
            &guest
        )
        .await
        .body,
        "second"
    );
});

database_test!(the_shelf_name_cannot_be_taken_at_the_root, harness, {
    let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
    harness
        .write(id, "elsewhere/Shared with me", b"fine below the root")
        .await;

    let written = put(&harness, "/v1/files/Shared with me", &bearer, b"x").await;
    let moved = move_to(
        &harness,
        &bearer,
        "elsewhere/Shared with me",
        "Shared with me",
    )
    .await;

    assert_eq!(written.status, StatusCode::CONFLICT, "{}", written.body);
    assert_eq!(moved.status, StatusCode::CONFLICT, "{}", moved.body);
});

database_test!(
    search_finds_shared_files_and_nothing_else_of_the_owners,
    harness,
    {
        let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
        let (guest_id, guest) = session(&harness, "guest@example.com", Role::Member).await;
        harness
            .write(owner, "family/photos/beach.jpg", b"sand")
            .await;
        harness
            .write(owner, "family/beach-diary.txt", b"private")
            .await;
        harness
            .write(owner, "family/photos/2019/beach-old.jpg", b"older")
            .await;
        harness.write(guest_id, "beach-mine.txt", b"mine").await;
        granted(
            &harness,
            &owner_bearer,
            "family/photos",
            "guest@example.com",
            "read",
        )
        .await;

        let found = get(&harness, "/v1/search?q=beach", &guest).await;
        let mut paths: Vec<String> = found
            .json()
            .as_array()
            .expect("hits")
            .iter()
            .map(|hit| hit["path"].as_str().unwrap_or_default().to_owned())
            .collect();
        paths.sort();

        assert_eq!(
            paths,
            [
                "Shared with me/photos/2019/beach-old.jpg",
                "Shared with me/photos/beach.jpg",
                "beach-mine.txt"
            ]
        );
    }
);

database_test!(
    a_shared_folder_the_owner_trashed_is_gone_for_the_grantee,
    harness,
    {
        let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
        let (_, guest) = session(&harness, "guest@example.com", Role::Member).await;
        harness.write(owner, "photos/beach.jpg", b"sand").await;
        granted(
            &harness,
            &owner_bearer,
            "photos",
            "guest@example.com",
            "read",
        )
        .await;

        harness.trash(&harness.resolve(owner, "photos").await).await;

        assert_eq!(
            get(&harness, "/v1/folders/Shared with me/photos", &guest)
                .await
                .status,
            StatusCode::NOT_FOUND
        );

        assert_eq!(
            get(
                &harness,
                "/v1/files/Shared with me/photos/beach.jpg",
                &guest
            )
            .await
            .status,
            StatusCode::NOT_FOUND
        );
        assert!(
            get(&harness, "/v1/grants/received", &guest)
                .await
                .json()
                .as_array()
                .is_some_and(Vec::is_empty)
        );
    }
);

database_test!(an_account_cannot_share_with_itself, harness, {
    let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
    harness.write(owner, "photos/beach.jpg", b"sand").await;

    let refused = grant(
        &harness,
        &owner_bearer,
        "photos",
        "Owner@Example.com",
        "read",
    )
    .await;

    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.body);
    assert!(
        refused.body.contains("different address"),
        "{}",
        refused.body
    );
    assert!(
        get(&harness, "/v1/folders", &owner_bearer)
            .await
            .names()
            .contains(&"photos".to_owned())
    );
});

database_test!(a_write_grant_on_a_file_grows_nothing_below_it, harness, {
    let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
    let (_, guest) = session(&harness, "guest@example.com", Role::Member).await;
    let file = harness.write(owner, "photos/beach.jpg", b"sand").await;
    granted(
        &harness,
        &owner_bearer,
        "photos/beach.jpg",
        "guest@example.com",
        "write",
    )
    .await;

    let written = put(
        &harness,
        "/v1/files/Shared with me/beach.jpg/a/b.txt",
        &guest,
        b"x",
    )
    .await;
    let overwritten = put(
        &harness,
        "/v1/files/Shared with me/beach.jpg",
        &guest,
        b"dune",
    )
    .await;

    assert_eq!(written.status, StatusCode::BAD_REQUEST, "{}", written.body);
    assert!(harness.children(&file).await.is_empty());
    assert_eq!(
        overwritten.status,
        StatusCode::CREATED,
        "{}",
        overwritten.body
    );
    assert_eq!(
        get(&harness, "/v1/files/Shared with me/beach.jpg", &guest)
            .await
            .body,
        "dune"
    );
});

database_test!(
    a_share_of_a_trashed_folder_stays_on_its_owners_list,
    harness,
    {
        let (owner, owner_bearer) = session(&harness, "owner@example.com", Role::Member).await;
        harness.account("guest@example.com", Role::Member).await;
        harness.write(owner, "photos/beach.jpg", b"sand").await;
        granted(
            &harness,
            &owner_bearer,
            "photos",
            "guest@example.com",
            "read",
        )
        .await;

        harness.trash(&harness.resolve(owner, "photos").await).await;

        let given = get(&harness, "/v1/grants", &owner_bearer).await.json();
        let given = given.as_array().expect("a list");
        assert_eq!(given.len(), 1);
        assert_eq!(given[0]["in_trash"], true);
    }
);
