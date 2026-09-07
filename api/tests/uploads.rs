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
    fn offset(&self) -> i64 {
        self.headers
            .get("upload-offset")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok())
            .unwrap_or(-1)
    }

    fn id(&self) -> String {
        self.body
            .split("\"id\":\"")
            .nth(1)
            .and_then(|rest| rest.split('"').next())
            .unwrap_or_else(|| panic!("no id in {}", self.body))
            .to_owned()
    }
}

async fn call(
    harness: &Harness,
    method: &str,
    path: &str,
    bearer: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Answer {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    for (name, value) in headers {
        request = request.header(*name, *value);
    }

    let response = build_router(harness.state.clone(), &[], None)
        .oneshot(
            request
                .body(Body::from(body))
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

async fn session(harness: &Harness, email: &str, role: Role) -> (Uuid, String) {
    let user = harness.account(email, role).await;
    let token = harness.state.sessions.issue(user.id).expect("a token");
    (user.id, token)
}

async fn begin(harness: &Harness, bearer: &str, path: &str, size: usize) -> Answer {
    call(
        harness,
        "POST",
        "/v1/uploads",
        bearer,
        &[],
        format!(r#"{{"path":"{path}","size":{size}}}"#).into_bytes(),
    )
    .await
}

async fn send(harness: &Harness, bearer: &str, id: &str, offset: usize, chunk: &[u8]) -> Answer {
    call(
        harness,
        "PATCH",
        &format!("/v1/uploads/{id}"),
        bearer,
        &[("upload-offset", &offset.to_string())],
        chunk.to_vec(),
    )
    .await
}

fn payload(size: usize) -> Vec<u8> {
    (0..size)
        .map(|index| u8::try_from(index % 251).expect("under 251"))
        .collect()
}

database_test!(an_interrupted_upload_resumes_to_the_same_digest, harness, {
    let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
    let whole = payload(300_000);

    let opened = begin(&harness, &bearer, "big.bin", whole.len()).await;
    assert_eq!(opened.status, StatusCode::CREATED, "{}", opened.body);
    let upload = opened.id();

    // The client sends a chunk, then dies. It does not know what arrived.
    let first = send(&harness, &bearer, &upload, 0, &whole[..120_000]).await;
    assert_eq!(first.status, StatusCode::OK);

    // On coming back it asks where it got to rather than guessing.
    let asked = call(
        &harness,
        "GET",
        &format!("/v1/uploads/{upload}"),
        &bearer,
        &[],
        Vec::new(),
    )
    .await;
    let resume = usize::try_from(asked.offset()).expect("a real offset");
    assert_eq!(resume, 120_000);

    send(&harness, &bearer, &upload, resume, &whole[resume..250_000]).await;
    send(&harness, &bearer, &upload, 250_000, &whole[250_000..]).await;

    let finished = call(
        &harness,
        "POST",
        &format!("/v1/uploads/{upload}/finish"),
        &bearer,
        &[],
        Vec::new(),
    )
    .await;
    assert_eq!(finished.status, StatusCode::CREATED, "{}", finished.body);

    // The digest of the resumed upload has to be the digest of the whole file, which is the
    // acceptance criterion and the only thing that makes resumption trustworthy.
    let node = harness.resolve(id, "big.bin").await;
    assert_eq!(
        node.size,
        i64::try_from(whole.len()).expect("a small payload")
    );
    assert_eq!(
        node.etag,
        roxycloud_core::node::etag_for_file(roxycloud_core::blob::BlobHash::from(blake3::hash(
            &whole
        )))
    );
});

database_test!(
    a_chunk_that_died_mid_body_does_not_corrupt_the_file,
    harness,
    {
        let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
        let whole = payload(200_000);
        let upload = begin(&harness, &bearer, "torn.bin", whole.len()).await.id();

        // A PATCH whose body stopped halfway leaves the file longer than the offset the session
        // recorded, because `received` is only written once a chunk has drained. Rolling the row back
        // is what that looks like from the next request's side.
        send(&harness, &bearer, &upload, 0, &whole[..150_000]).await;
        harness.rewind_upload(&upload, 100_000).await;

        let asked = call(
            &harness,
            "GET",
            &format!("/v1/uploads/{upload}"),
            &bearer,
            &[],
            Vec::new(),
        )
        .await;
        let resume = usize::try_from(asked.offset()).expect("a real offset");
        assert_eq!(resume, 100_000);

        send(&harness, &bearer, &upload, resume, &whole[resume..]).await;
        let finished = call(
            &harness,
            "POST",
            &format!("/v1/uploads/{upload}/finish"),
            &bearer,
            &[],
            Vec::new(),
        )
        .await;

        assert_eq!(finished.status, StatusCode::CREATED, "{}", finished.body);
        let node = harness.resolve(id, "torn.bin").await;
        assert_eq!(
            node.etag,
            roxycloud_core::node::etag_for_file(roxycloud_core::blob::BlobHash::from(
                blake3::hash(&whole)
            )),
            "appending after the bytes a dead request left would duplicate a region and lose the tail"
        );
    }
);

database_test!(
    a_chunk_at_the_wrong_offset_is_told_the_right_one,
    harness,
    {
        let (_, bearer) = session(&harness, "owner@example.com", Role::Member).await;
        let whole = payload(1000);
        let upload = begin(&harness, &bearer, "notes.bin", whole.len())
            .await
            .id();
        send(&harness, &bearer, &upload, 0, &whole[..400]).await;

        // A client that resends a chunk it already sent would otherwise write it twice.
        let repeated = send(&harness, &bearer, &upload, 0, &whole[..400]).await;

        assert_eq!(repeated.status, StatusCode::CONFLICT);
        assert_eq!(
            repeated.offset(),
            400,
            "the refusal carries the offset to resume from, or the client needs another round trip"
        );
    }
);

database_test!(an_upload_cannot_grow_past_what_it_promised, harness, {
    let (_, bearer) = session(&harness, "owner@example.com", Role::Member).await;
    let upload = begin(&harness, &bearer, "small.bin", 100).await.id();

    let overflowing = send(&harness, &bearer, &upload, 0, &payload(200)).await;

    assert_eq!(overflowing.status, StatusCode::BAD_REQUEST);
    let gone = call(
        &harness,
        "GET",
        &format!("/v1/uploads/{upload}"),
        &bearer,
        &[],
        Vec::new(),
    )
    .await;
    assert_eq!(
        gone.status,
        StatusCode::NOT_FOUND,
        "a session that was lied to about its size has nothing left to resume"
    );
});

database_test!(an_upload_short_of_its_size_will_not_finish, harness, {
    let (_, bearer) = session(&harness, "owner@example.com", Role::Member).await;
    let whole = payload(1000);
    let upload = begin(&harness, &bearer, "half.bin", whole.len()).await.id();
    send(&harness, &bearer, &upload, 0, &whole[..400]).await;

    let early = call(
        &harness,
        "POST",
        &format!("/v1/uploads/{upload}/finish"),
        &bearer,
        &[],
        Vec::new(),
    )
    .await;

    assert_eq!(early.status, StatusCode::BAD_REQUEST);
    assert!(early.body.contains("400"), "{}", early.body);
});

database_test!(a_session_belongs_to_the_account_that_opened_it, harness, {
    let (_, mine) = session(&harness, "mine@example.com", Role::Member).await;
    let (_, theirs) = session(&harness, "theirs@example.com", Role::Member).await;
    let upload = begin(&harness, &mine, "private.bin", 10).await.id();

    for (method, suffix) in [("GET", ""), ("DELETE", "")] {
        let answer = call(
            &harness,
            method,
            &format!("/v1/uploads/{upload}{suffix}"),
            &theirs,
            &[],
            Vec::new(),
        )
        .await;
        assert_eq!(answer.status, StatusCode::NOT_FOUND, "{method}");
    }
});

database_test!(
    there_is_no_point_starting_an_upload_that_cannot_fit,
    harness,
    {
        let (id, bearer) = session(&harness, "owner@example.com", Role::Member).await;
        harness.root(id).await;
        harness.set_quota(id, 500).await;

        let opened = begin(&harness, &bearer, "enormous.bin", 100_000).await;

        assert_eq!(
            opened.status,
            StatusCode::INSUFFICIENT_STORAGE,
            "refusing at the end would mean sending a file for an hour to be told there was no room"
        );
    }
);

database_test!(a_reader_may_not_start_an_upload, harness, {
    let (_, bearer) = session(&harness, "reader@example.com", Role::Reader).await;

    let opened = begin(&harness, &bearer, "theirs.bin", 10).await;

    assert_eq!(opened.status, StatusCode::FORBIDDEN);
});

database_test!(abandoning_a_session_takes_its_staged_bytes, harness, {
    let (_, bearer) = session(&harness, "owner@example.com", Role::Member).await;
    let upload = begin(&harness, &bearer, "dropped.bin", 1000).await.id();
    send(&harness, &bearer, &upload, 0, &payload(400)).await;
    assert_eq!(harness.staged_uploads().await, 1);

    let dropped = call(
        &harness,
        "DELETE",
        &format!("/v1/uploads/{upload}"),
        &bearer,
        &[],
        Vec::new(),
    )
    .await;

    assert_eq!(dropped.status, StatusCode::NO_CONTENT);
    assert_eq!(harness.staged_uploads().await, 0);
});

database_test!(a_session_nobody_came_back_to_is_swept, harness, {
    let (_, bearer) = session(&harness, "owner@example.com", Role::Member).await;
    let upload = begin(&harness, &bearer, "forgotten.bin", 1000).await.id();
    send(&harness, &bearer, &upload, 0, &payload(400)).await;

    harness.expire_uploads().await;
    let expired = roxycloud_api::uploads::purge_expired(&harness.state.db)
        .await
        .expect("purging");
    let live = roxycloud_api::uploads::live_staged(&harness.state.db)
        .await
        .expect("listing");
    let swept = harness
        .state
        .staging
        .sweep(&live, std::time::Duration::ZERO)
        .await
        .expect("sweeping the staging area");

    assert_eq!(expired.len(), 1);
    assert_eq!(swept, 1, "the row going is not the same as the bytes going");
    assert_eq!(harness.staged_uploads().await, 0);
});
