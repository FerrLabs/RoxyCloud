mod common;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use bytes::Bytes;
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
    a_session_being_written_to_refuses_a_second_write,
    harness,
    {
        let (_, bearer) = session(&harness, "owner@example.com", Role::Member).await;
        let whole = payload(1000);
        let upload = begin(&harness, &bearer, "raced.bin", whole.len())
            .await
            .id();

        // Standing in for a request that is still draining its body: two writers do not share a file
        // cursor, so the second truncating under the first would leave a hole of zeros between their
        // write heads, and a length landing on `size` would be stored under an ETag over those zeros.
        // Forcing that interleave in process is not reliable, so what is pinned here is the claim.
        harness.hold_upload(&upload).await;
        let refused = send(&harness, &bearer, &upload, 0, &whole[..400]).await;

        harness.release_upload(&upload).await;
        let allowed = send(&harness, &bearer, &upload, 0, &whole[..400]).await;

        assert_eq!(refused.status, StatusCode::CONFLICT);
        assert_eq!(allowed.status, StatusCode::OK);
        assert_eq!(allowed.offset(), 400);
    }
);

database_test!(a_writer_whose_claim_lapsed_records_nothing, harness, {
    let (id, _) = session(&harness, "owner@example.com", Role::Member).await;
    let opened = roxycloud_api::uploads::begin(
        &harness.state.db,
        &harness.state.staging,
        id,
        "lapsed.bin",
        1000,
    )
    .await
    .expect("a session");

    // A body slower than the claim, driven at this level because the race cannot be produced
    // through two HTTP requests in one task. The stream stops after its first chunk, the claim is
    // handed to somebody else, and only then does the write finish.
    let (tell, waiting) = tokio::sync::oneshot::channel::<()>();
    let (resume, wait) = tokio::sync::oneshot::channel::<()>();
    let stopped = std::sync::Arc::new(tokio::sync::Mutex::new(Some(tell)));
    let held = std::sync::Arc::new(tokio::sync::Mutex::new(Some(wait)));
    let body = futures::stream::unfold(0u8, move |step| {
        let stopped = stopped.clone();
        let held = held.clone();
        async move {
            match step {
                0 => Some((Ok::<_, std::io::Error>(Bytes::from_static(b"first")), 1)),
                1 => {
                    if let Some(tell) = stopped.lock().await.take() {
                        tell.send(()).ok();
                    }
                    if let Some(wait) = held.lock().await.take() {
                        wait.await.ok();
                    }
                    Some((Ok(Bytes::from_static(b"second")), 2))
                }
                _ => None,
            }
        }
    });

    let sending = roxycloud_api::uploads::append(
        &harness.state.db,
        &harness.state.staging,
        &opened,
        0,
        Box::pin(body),
    );
    let stealing = async {
        waiting.await.ok();
        harness.steal_claim(opened.id).await;
        resume.send(()).ok();
    };

    let (outcome, ()) = tokio::join!(sending, stealing);

    assert!(
        matches!(outcome, Err(roxycloud_api::error::ApiError::AlreadyWriting)),
        "a writer that lost its claim has to record nothing"
    );
    let after = roxycloud_api::uploads::of(&harness.state.db, id, opened.id)
        .await
        .expect("the session");
    assert_eq!(
        after.received, 0,
        "recording here would count the zeros between two write heads as arrived"
    );
});

database_test!(
    a_writer_that_lost_its_claim_takes_nothing_with_it,
    harness,
    {
        let (id, _) = session(&harness, "owner@example.com", Role::Member).await;
        let opened = roxycloud_api::uploads::begin(
            &harness.state.db,
            &harness.state.staging,
            id,
            "oversent.bin",
            8,
        )
        .await
        .expect("a session");

        // The one thing a writer without the claim could still do: over-send, and take the row and the
        // staged file away from whoever holds the session now, who would meet a 404 on their next chunk
        // for a rule somebody else broke.
        let (tell, waiting) = tokio::sync::oneshot::channel::<()>();
        let (resume, wait) = tokio::sync::oneshot::channel::<()>();
        let stopped = std::sync::Arc::new(tokio::sync::Mutex::new(Some(tell)));
        let held = std::sync::Arc::new(tokio::sync::Mutex::new(Some(wait)));
        let body = futures::stream::unfold(0u8, move |step| {
            let stopped = stopped.clone();
            let held = held.clone();
            async move {
                match step {
                    0 => Some((Ok::<_, std::io::Error>(Bytes::from_static(b"first")), 1)),
                    1 => {
                        if let Some(tell) = stopped.lock().await.take() {
                            tell.send(()).ok();
                        }
                        if let Some(wait) = held.lock().await.take() {
                            wait.await.ok();
                        }
                        Some((Ok(Bytes::from_static(b"past the size")), 2))
                    }
                    _ => None,
                }
            }
        });

        let sending = roxycloud_api::uploads::append(
            &harness.state.db,
            &harness.state.staging,
            &opened,
            0,
            Box::pin(body),
        );
        let stealing = async {
            waiting.await.ok();
            harness.steal_claim(opened.id).await;
            resume.send(()).ok();
        };

        let (outcome, ()) = tokio::join!(sending, stealing);

        assert!(
            matches!(outcome, Err(roxycloud_api::error::ApiError::AlreadyWriting)),
            "an over-send by a writer without the claim is refused for the claim, not for the size"
        );
        assert!(
            roxycloud_api::uploads::of(&harness.state.db, id, opened.id)
                .await
                .is_ok(),
            "the session belongs to whoever holds it now, not to the writer that lost it"
        );
    }
);

database_test!(a_writer_that_lost_its_claim_stops_reading, harness, {
    let (id, _) = session(&harness, "owner@example.com", Role::Member).await;
    let opened = roxycloud_api::uploads::begin(
        &harness.state.db,
        &harness.state.staging,
        id,
        "stalled.bin",
        1000,
    )
    .await
    .expect("a session");

    // A body that stalls for longer than the claim, which is the case this feature exists for. The
    // stream stops after its first chunk and never resumes, so the only thing that can end the
    // write is the renewal noticing the session went to somebody else.
    let (tell, waiting) = tokio::sync::oneshot::channel::<()>();
    let stopped = std::sync::Arc::new(tokio::sync::Mutex::new(Some(tell)));
    let read = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = read.clone();
    let body = futures::stream::unfold(0u8, move |step| {
        let stopped = stopped.clone();
        let counted = counted.clone();
        async move {
            match step {
                0 => {
                    counted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    Some((Ok::<_, std::io::Error>(Bytes::from_static(b"first")), 1))
                }
                1 => {
                    if let Some(tell) = stopped.lock().await.take() {
                        tell.send(()).ok();
                    }
                    std::future::pending::<()>().await;
                    unreachable!()
                }
                _ => None,
            }
        }
    });

    let db = harness.state.db.clone();
    let staging = harness.state.staging.clone();
    let session = opened.clone();
    let sending = tokio::spawn(async move {
        roxycloud_api::uploads::append(&db, &staging, &session, 0, Box::pin(body)).await
    });

    waiting.await.ok();
    harness.steal_claim(opened.id).await;

    // Standing in for the claim running out under a body still draining. The clock only moves for
    // the renewal timer, and only once the write is already stalled and the session already gone.
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(310)).await;
    tokio::time::resume();

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), sending)
        .await
        .expect("a writer that lost its claim has to stop rather than wait out its body")
        .expect("the write task");

    assert!(
        matches!(outcome, Err(roxycloud_api::error::ApiError::AlreadyWriting)),
        "a writer that lost its claim has to be refused"
    );
    assert_eq!(
        read.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "reading on would dribble bytes into the region the new holder is recording"
    );
});

database_test!(
    a_claim_nobody_released_does_not_strand_the_session,
    harness,
    {
        let (_, bearer) = session(&harness, "owner@example.com", Role::Member).await;
        let upload = begin(&harness, &bearer, "abandoned.bin", 1000).await.id();

        // A request that died holding the claim would otherwise hold the session for the day it has
        // left, so the claim expires rather than needing anybody to come back for it.
        harness.hold_upload(&upload).await;
        harness.expire_upload_claims().await;
        let after = send(&harness, &bearer, &upload, 0, &payload(400)).await;

        assert_eq!(after.status, StatusCode::OK);
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

database_test!(sessions_opened_at_once_do_not_all_get_through, harness, {
    let (_, bearer) = session(&harness, "owner@example.com", Role::Member).await;

    let paths: Vec<String> = (0..20).map(|which| format!("burst-{which}.bin")).collect();
    let answers =
        futures::future::join_all(paths.iter().map(|path| begin(&harness, &bearer, path, 10)))
            .await;

    let opened = answers
        .iter()
        .filter(|answer| answer.status == StatusCode::CREATED)
        .count();

    assert_eq!(
        opened, 8,
        "counting and inserting without a lock lets every concurrent request read the same count"
    );
});

database_test!(one_account_cannot_hold_every_session_open, harness, {
    let (_, bearer) = session(&harness, "owner@example.com", Role::Member).await;

    for opened in 1..=8 {
        let answer = begin(&harness, &bearer, &format!("file-{opened}.bin"), 10).await;
        assert_eq!(answer.status, StatusCode::CREATED, "session {opened}");
    }
    let refused = begin(&harness, &bearer, "one-too-many.bin", 10).await;

    // Hitting the ceiling has to be something a client can act on rather than wait out.
    let listed = call(&harness, "GET", "/v1/uploads", &bearer, &[], Vec::new()).await;
    assert_eq!(
        listed.body.matches("\"path\"").count(),
        8,
        "{}",
        listed.body
    );

    let first = listed.id();
    let dropped = call(
        &harness,
        "DELETE",
        &format!("/v1/uploads/{first}"),
        &bearer,
        &[],
        Vec::new(),
    )
    .await;
    assert_eq!(dropped.status, StatusCode::NO_CONTENT);
    let after = begin(&harness, &bearer, "room-again.bin", 10).await;

    assert_eq!(after.status, StatusCode::CREATED, "{}", after.body);
    assert_eq!(
        refused.status,
        StatusCode::TOO_MANY_REQUESTS,
        "the room check when a session opens is not a reservation, so without a ceiling one          account can stage its whole quota once per session"
    );
});

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
