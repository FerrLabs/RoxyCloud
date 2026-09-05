mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use roxycloud_api::build_router;
use roxycloud_core::role::Role;
use tower::ServiceExt;
use uuid::Uuid;

use common::Harness;

async fn credential(harness: &Harness, email: &str, role: Role) -> (Uuid, String) {
    let user = harness.account(email, role).await;
    let mut tx = harness.state.db.begin().await.expect("begin");
    let minted = roxycloud_api::app_passwords::mint(&mut tx, user.id, "a client")
        .await
        .expect("minting");
    tx.commit().await.expect("commit");

    let basic = base64(format!("{email}:{}", minted.secret).as_bytes());
    (user.id, format!("Basic {basic}"))
}

fn base64(raw: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in raw.chunks(3) {
        let mut buffer = [0u8; 3];
        buffer[..chunk.len()].copy_from_slice(chunk);
        let bits =
            (u32::from(buffer[0]) << 16) | (u32::from(buffer[1]) << 8) | u32::from(buffer[2]);
        for index in 0..4 {
            if index <= chunk.len() {
                let value = (bits >> (18 - index * 6)) & 0x3f;
                out.push(char::from(ALPHABET[value as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

struct Answer {
    status: StatusCode,
    headers: header::HeaderMap,
    body: String,
}

async fn dav(
    harness: &Harness,
    method: &str,
    path: &str,
    auth: &str,
    extra: &[(&str, &str)],
    body: &str,
) -> Answer {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, auth);
    for (name, value) in extra {
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

database_test!(a_client_without_a_credential_is_asked_for_one, harness, {
    let answer = dav(&harness, "OPTIONS", "/dav", "", &[], "").await;

    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert!(
        answer
            .headers
            .get(header::WWW_AUTHENTICATE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("Basic")),
        "a client that is not challenged never asks the user for anything"
    );
});

database_test!(a_session_token_is_not_a_webdav_credential, harness, {
    let user = harness.account("session@example.com", Role::Member).await;
    let token = harness.state.sessions.issue(user.id).expect("a token");

    let answer = dav(
        &harness,
        "OPTIONS",
        "/dav",
        &format!("Bearer {token}"),
        &[],
        "",
    )
    .await;

    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
});

database_test!(options_advertises_what_the_surface_supports, harness, {
    let (_, auth) = credential(&harness, "options@example.com", Role::Member).await;

    let answer = dav(&harness, "OPTIONS", "/dav", &auth, &[], "").await;

    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    assert_eq!(
        answer.headers.get("dav").and_then(|v| v.to_str().ok()),
        Some("1"),
        "class 1 until locking lands, and clients read this before they trust anything else"
    );
    let allow = answer
        .headers
        .get(header::ALLOW)
        .and_then(|v| v.to_str().ok())
        .expect("an Allow header");
    for method in ["PROPFIND", "MKCOL", "COPY", "MOVE", "PUT", "DELETE"] {
        assert!(allow.contains(method), "{method} missing from {allow}");
    }
});

database_test!(
    propfind_describes_the_collection_and_its_children,
    harness,
    {
        let (owner, auth) = credential(&harness, "listing@example.com", Role::Member).await;
        harness.write(owner, "photos/summer.jpg", b"bytes").await;

        let answer = dav(&harness, "PROPFIND", "/dav", &auth, &[("depth", "1")], "").await;

        assert_eq!(answer.status, StatusCode::MULTI_STATUS);
        assert!(
            answer
                .headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|value| value.contains("xml")),
            "a client that cannot tell this is XML will not parse it"
        );
        assert!(
            answer.body.contains("<D:href>/dav/</D:href>"),
            "{}",
            answer.body
        );
        assert!(
            answer.body.contains("<D:href>/dav/photos/</D:href>"),
            "a collection href ends in a slash: {}",
            answer.body
        );
        assert!(
            answer.body.contains("<D:collection/>"),
            "without a resourcetype a client shows a file where a folder is: {}",
            answer.body
        );
        assert!(answer.body.contains("quota-used-bytes"), "{}", answer.body);
    }
);

database_test!(
    propfind_at_depth_zero_stops_at_the_collection_itself,
    harness,
    {
        let (owner, auth) = credential(&harness, "depth@example.com", Role::Member).await;
        harness.write(owner, "photos/summer.jpg", b"bytes").await;

        let answer = dav(&harness, "PROPFIND", "/dav", &auth, &[("depth", "0")], "").await;

        assert!(answer.body.contains("<D:href>/dav/</D:href>"));
        assert!(!answer.body.contains("photos"), "{}", answer.body);
    }
);

database_test!(propfind_refuses_an_infinite_walk, harness, {
    let (_, auth) = credential(&harness, "infinite@example.com", Role::Member).await;

    let answer = dav(
        &harness,
        "PROPFIND",
        "/dav",
        &auth,
        &[("depth", "infinity")],
        "",
    )
    .await;

    assert_eq!(answer.status, StatusCode::FORBIDDEN);
    assert!(
        answer.body.contains("propfind-finite-depth"),
        "the refusal has to say why, or a client retries it forever: {}",
        answer.body
    );
});

database_test!(
    propfind_answers_a_named_list_and_reports_what_it_lacks,
    harness,
    {
        let (owner, auth) = credential(&harness, "named@example.com", Role::Member).await;
        harness.write(owner, "a.txt", b"bytes").await;

        let answer = dav(
            &harness,
            "PROPFIND",
            "/dav/a.txt",
            &auth,
            &[("depth", "0")],
            r#"<D:propfind xmlns:D="DAV:"><D:prop><D:getetag/><D:author/></D:prop></D:propfind>"#,
        )
        .await;

        assert!(answer.body.contains("<D:getetag>"), "{}", answer.body);
        assert!(
            answer.body.contains("HTTP/1.1 404 Not Found"),
            "a property we do not keep is reported missing rather than silently dropped: {}",
            answer.body
        );
        assert!(
            !answer.body.contains("quota-used-bytes"),
            "a named list is not an invitation to send everything: {}",
            answer.body
        );
    }
);

database_test!(
    a_name_that_looks_like_markup_cannot_break_the_document,
    harness,
    {
        let (owner, auth) = credential(&harness, "markup@example.com", Role::Member).await;
        harness.write(owner, "a<b>&c.txt", b"bytes").await;

        let answer = dav(&harness, "PROPFIND", "/dav", &auth, &[("depth", "1")], "").await;

        assert!(
            answer.body.contains("a&lt;b&gt;&amp;c.txt"),
            "{}",
            answer.body
        );
        assert!(!answer.body.contains("<b>"), "{}", answer.body);
    }
);

database_test!(
    mkcol_creates_a_collection_and_says_so_the_second_time,
    harness,
    {
        let (owner, auth) = credential(&harness, "mkcol@example.com", Role::Member).await;

        let created = dav(&harness, "MKCOL", "/dav/photos", &auth, &[], "").await;
        let again = dav(&harness, "MKCOL", "/dav/photos", &auth, &[], "").await;
        let orphan = dav(&harness, "MKCOL", "/dav/absent/deep", &auth, &[], "").await;

        assert_eq!(created.status, StatusCode::CREATED);
        assert_eq!(again.status, StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(
            orphan.status,
            StatusCode::CONFLICT,
            "MKCOL does not invent the collections above the one it was asked for"
        );
        let root = harness.root(owner).await;
        assert_eq!(harness.children(&root).await, ["photos"]);
    }
);

database_test!(
    put_writes_a_file_and_refuses_to_invent_its_folder,
    harness,
    {
        let (owner, auth) = credential(&harness, "put@example.com", Role::Member).await;

        let created = dav(&harness, "PUT", "/dav/notes.md", &auth, &[], "first").await;
        let replaced = dav(&harness, "PUT", "/dav/notes.md", &auth, &[], "second").await;
        let orphan = dav(&harness, "PUT", "/dav/absent/notes.md", &auth, &[], "third").await;

        assert_eq!(created.status, StatusCode::CREATED);
        assert_eq!(replaced.status, StatusCode::NO_CONTENT);
        assert_eq!(orphan.status, StatusCode::CONFLICT);
        assert!(created.headers.contains_key(header::ETAG));

        let read = dav(&harness, "GET", "/dav/notes.md", &auth, &[], "").await;
        assert_eq!(read.body, "second");
        let root = harness.root(owner).await;
        assert_eq!(harness.children(&root).await, ["notes.md"]);
    }
);

database_test!(a_reader_may_walk_the_tree_and_nothing_else, harness, {
    let (owner, auth) = credential(&harness, "reader@example.com", Role::Reader).await;
    harness.write(owner, "a.txt", b"already here").await;

    let listed = dav(&harness, "PROPFIND", "/dav", &auth, &[("depth", "1")], "").await;
    let read = dav(&harness, "GET", "/dav/a.txt", &auth, &[], "").await;
    let written = dav(&harness, "PUT", "/dav/b.txt", &auth, &[], "nope").await;
    let deleted = dav(&harness, "DELETE", "/dav/a.txt", &auth, &[], "").await;
    let collection = dav(&harness, "MKCOL", "/dav/photos", &auth, &[], "").await;

    assert_eq!(listed.status, StatusCode::MULTI_STATUS);
    assert_eq!(read.body, "already here");
    assert_eq!(written.status, StatusCode::FORBIDDEN);
    assert_eq!(deleted.status, StatusCode::FORBIDDEN);
    assert_eq!(collection.status, StatusCode::FORBIDDEN);
});

database_test!(delete_moves_a_node_to_the_trash, harness, {
    let (owner, auth) = credential(&harness, "delete@example.com", Role::Member).await;
    harness.write(owner, "photos/x.jpg", b"bytes").await;

    let answer = dav(&harness, "DELETE", "/dav/photos", &auth, &[], "").await;

    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    let root = harness.root(owner).await;
    assert!(harness.children(&root).await.is_empty());
    assert_eq!(harness.trashed(owner).await, ["photos"]);
});

database_test!(move_takes_the_node_and_its_contents, harness, {
    let (owner, auth) = credential(&harness, "move@example.com", Role::Member).await;
    harness.write(owner, "photos/x.jpg", b"bytes").await;
    harness
        .write(owner, "archive/keep.txt", b"already here")
        .await;

    let answer = dav(
        &harness,
        "MOVE",
        "/dav/photos",
        &auth,
        &[("destination", "/dav/archive/photos")],
        "",
    )
    .await;

    assert_eq!(answer.status, StatusCode::CREATED);
    let moved = harness.resolve(owner, "archive/photos").await;
    assert_eq!(harness.children(&moved).await, ["x.jpg"]);
});

database_test!(
    copy_shares_the_bytes_rather_than_writing_them_again,
    harness,
    {
        let (owner, auth) = credential(&harness, "copy@example.com", Role::Member).await;
        let contents = b"one blob, two nodes";
        harness.write(owner, "photos/x.jpg", contents).await;

        let answer = dav(
            &harness,
            "COPY",
            "/dav/photos",
            &auth,
            &[("destination", "/dav/backup")],
            "",
        )
        .await;

        assert_eq!(answer.status, StatusCode::CREATED);
        let copy = harness.resolve(owner, "backup").await;
        assert_eq!(harness.children(&copy).await, ["x.jpg"]);
        assert_eq!(
            harness.blob(blake3::hash(contents).into()).await,
            Some((2, false)),
            "a copy is a second reference, not a second upload"
        );
        let original = harness.resolve(owner, "photos/x.jpg").await;
        assert_eq!(original.size, i64::try_from(contents.len()).expect("small"));
    }
);

database_test!(overwrite_is_honoured_in_both_directions, harness, {
    let (owner, auth) = credential(&harness, "overwrite@example.com", Role::Member).await;
    harness.write(owner, "a.txt", b"the source").await;
    harness.write(owner, "b.txt", b"the occupant").await;

    let refused = dav(
        &harness,
        "COPY",
        "/dav/a.txt",
        &auth,
        &[("destination", "/dav/b.txt"), ("overwrite", "F")],
        "",
    )
    .await;
    assert_eq!(refused.status, StatusCode::PRECONDITION_FAILED);

    let replaced = dav(
        &harness,
        "COPY",
        "/dav/a.txt",
        &auth,
        &[("destination", "/dav/b.txt")],
        "",
    )
    .await;

    assert_eq!(replaced.status, StatusCode::NO_CONTENT);
    let read = dav(&harness, "GET", "/dav/b.txt", &auth, &[], "").await;
    assert_eq!(read.body, "the source");
    let _ = owner;
});

database_test!(a_destination_whose_folder_is_missing_conflicts, harness, {
    let (owner, auth) = credential(&harness, "nowhere@example.com", Role::Member).await;
    harness.write(owner, "a.txt", b"bytes").await;

    let answer = dav(
        &harness,
        "MOVE",
        "/dav/a.txt",
        &auth,
        &[("destination", "/dav/absent/a.txt")],
        "",
    )
    .await;

    assert_eq!(answer.status, StatusCode::CONFLICT);
});

database_test!(proppatch_refuses_rather_than_pretending, harness, {
    let (owner, auth) = credential(&harness, "props@example.com", Role::Member).await;
    harness.write(owner, "a.txt", b"bytes").await;

    let answer = dav(
        &harness,
        "PROPPATCH",
        "/dav/a.txt",
        &auth,
        &[],
        r#"<D:propertyupdate xmlns:D="DAV:"><D:set><D:prop><D:author>me</D:author></D:prop></D:set></D:propertyupdate>"#,
    )
    .await;

    assert_eq!(answer.status, StatusCode::MULTI_STATUS);
    assert!(
        answer.body.contains("403 Forbidden"),
        "nothing stores dead properties, and a 200 here would be a lie: {}",
        answer.body
    );
});

database_test!(
    one_account_never_sees_another_through_this_surface,
    harness,
    {
        let (mine, _) = credential(&harness, "mine@example.com", Role::Member).await;
        let (_, theirs) = credential(&harness, "theirs@example.com", Role::Member).await;
        harness.write(mine, "private.txt", b"not yours").await;

        let listed = dav(&harness, "PROPFIND", "/dav", &theirs, &[("depth", "1")], "").await;
        let read = dav(&harness, "GET", "/dav/private.txt", &theirs, &[], "").await;

        assert!(!listed.body.contains("private.txt"), "{}", listed.body);
        assert_eq!(read.status, StatusCode::NOT_FOUND);
    }
);

database_test!(a_path_that_climbs_out_of_the_tree_is_refused, harness, {
    let (_, auth) = credential(&harness, "climb@example.com", Role::Member).await;

    let answer = dav(
        &harness,
        "PROPFIND",
        "/dav/../../etc",
        &auth,
        &[("depth", "0")],
        "",
    )
    .await;

    assert_ne!(answer.status, StatusCode::MULTI_STATUS);
});

database_test!(a_move_onto_the_folder_holding_it_is_refused, harness, {
    let (owner, auth) = credential(&harness, "ancestor@example.com", Role::Member).await;
    harness
        .write(owner, "a/b.txt", b"the file that must survive")
        .await;

    let answer = dav(
        &harness,
        "MOVE",
        "/dav/a/b.txt",
        &auth,
        &[("destination", "/dav/a")],
        "",
    )
    .await;

    assert_eq!(
        answer.status,
        StatusCode::FORBIDDEN,
        "replacing the folder would trash the file inside it on the way there"
    );
    let read = dav(&harness, "GET", "/dav/a/b.txt", &auth, &[], "").await;
    assert_eq!(read.body, "the file that must survive");
});

database_test!(a_copy_onto_the_folder_holding_it_is_refused, harness, {
    let (owner, auth) = credential(&harness, "ancestorcopy@example.com", Role::Member).await;
    harness
        .write(owner, "a/b.txt", b"still here afterwards")
        .await;

    let answer = dav(
        &harness,
        "COPY",
        "/dav/a/b.txt",
        &auth,
        &[("destination", "/dav/a")],
        "",
    )
    .await;

    assert_eq!(answer.status, StatusCode::FORBIDDEN);
    let read = dav(&harness, "GET", "/dav/a/b.txt", &auth, &[], "").await;
    assert_eq!(read.body, "still here afterwards");
});

database_test!(bytes_from_a_refused_upload_are_collectable, harness, {
    let (_, auth) = credential(&harness, "orphan@example.com", Role::Member).await;
    let contents = b"uploaded into a folder that is not there";

    let answer = dav(
        &harness,
        "PUT",
        "/dav/absent/x.txt",
        &auth,
        &[],
        "uploaded into a folder that is not there",
    )
    .await;

    assert_eq!(answer.status, StatusCode::CONFLICT);
    assert_eq!(
        harness.blob(blake3::hash(contents).into()).await,
        Some((0, true)),
        "the bytes reached the disk before the refusal, so something has to know they are there"
    );
    assert!(
        harness
            .blob_file_exists(blake3::hash(contents).into())
            .await
    );
});

database_test!(
    a_property_from_another_namespace_is_answered_under_it,
    harness,
    {
        let (owner, auth) = credential(&harness, "namespaced@example.com", Role::Member).await;
        harness.write(owner, "a.txt", b"bytes").await;

        let answer = dav(
        &harness,
        "PROPFIND",
        "/dav/a.txt",
        &auth,
        &[("depth", "0")],
        r#"<D:propfind xmlns:D="DAV:" xmlns:Z="urn:schemas-microsoft-com:"><D:prop><Z:Win32LastModifiedTime/></D:prop></D:propfind>"#,
    )
    .await;

        assert!(
            answer
                .body
                .contains(r#"xmlns:ns="urn:schemas-microsoft-com:""#),
            "a client matching on namespace has to recognise its own question: {}",
            answer.body
        );
        assert!(
            answer.body.contains("Win32LastModifiedTime"),
            "{}",
            answer.body
        );
    }
);

database_test!(
    a_reader_cannot_set_properties_and_a_ghost_has_none,
    harness,
    {
        let (owner, reader) = credential(&harness, "propreader@example.com", Role::Reader).await;
        harness.write(owner, "a.txt", b"bytes").await;
        let (_, member) = credential(&harness, "propmember@example.com", Role::Member).await;

        let refused = dav(&harness, "PROPPATCH", "/dav/a.txt", &reader, &[], "").await;
        let absent = dav(&harness, "PROPPATCH", "/dav/nowhere.txt", &member, &[], "").await;

        assert_eq!(refused.status, StatusCode::FORBIDDEN);
        assert_eq!(
            absent.status,
            StatusCode::NOT_FOUND,
            "properties on nothing is not a 207"
        );
    }
);
