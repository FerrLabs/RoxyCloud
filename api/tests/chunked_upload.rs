mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use stashden_client::Remote;
use stashden_client::remote::RemoteError;
use stashden_client::sync::Expect;
use stashden_core::role::Role;
use stashden_core::user::User;

use common::{Harness, serve};

const CHUNK: u64 = 1000;

fn contents(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from((index * 7) % 251).expect("under 251"))
        .collect()
}

async fn connect(harness: &Harness, account: &User) -> Remote {
    let base = serve(harness.state.clone()).await;
    let token = harness.session(account.id).await;
    Remote::new(&base, token).expect("a client")
}

async fn connect_losing_answer_of_patch(
    harness: &Harness,
    account: &User,
    nth: usize,
) -> (Remote, Arc<AtomicUsize>) {
    let patches = Arc::new(AtomicUsize::new(0));
    let counted = patches.clone();
    let router = stashden_api::build_router(harness.state.clone(), &[], None).layer(
        middleware::from_fn(move |request: Request, next: Next| {
            let counted = counted.clone();
            async move {
                let patch = request.method() == Method::PATCH;
                let response: Response = next.run(request).await;
                if patch && counted.fetch_add(1, Ordering::SeqCst) + 1 == nth {
                    return StatusCode::BAD_GATEWAY.into_response();
                }
                response
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port");
    let address = listener.local_addr().expect("a bound address");
    tokio::spawn(async move { axum::serve(listener, router).await.expect("serving") });
    let token = harness.session(account.id).await;
    let remote = Remote::new(&format!("http://{address}"), token).expect("a client");
    (remote, patches)
}

fn source_of(bytes: &[u8]) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().expect("a scratch file");
    std::fs::write(file.path(), bytes).expect("writing the source");
    file
}

database_test!(a_file_larger_than_a_chunk_arrives_intact, harness, {
    let owner = harness.account("chunks@example.com", Role::Member).await;
    let remote = connect(&harness, &owner).await;
    let bytes = contents(4500);
    let source = source_of(&bytes);

    let node = remote
        .upload_chunked("big.bin", source.path(), None, CHUNK)
        .await
        .expect("uploading");

    assert_eq!(node.size, 4500);
    assert_eq!(
        &remote.read("big.bin").await.expect("reading")[..],
        &bytes[..]
    );
    assert_eq!(harness.staged_uploads().await, 0, "nothing is left staged");
});

database_test!(a_file_that_fits_one_chunk_is_a_single_request, harness, {
    let owner = harness.account("single@example.com", Role::Member).await;
    let (remote, patches) = connect_losing_answer_of_patch(&harness, &owner, usize::MAX).await;
    let bytes = contents(800);
    let source = source_of(&bytes);

    remote
        .upload_chunked("small.bin", source.path(), None, CHUNK)
        .await
        .expect("uploading");

    assert_eq!(patches.load(Ordering::SeqCst), 0, "no session was opened");
    assert_eq!(
        &remote.read("small.bin").await.expect("reading")[..],
        &bytes[..]
    );
});

database_test!(a_chunk_whose_answer_is_lost_is_not_sent_twice, harness, {
    let owner = harness.account("flaky@example.com", Role::Member).await;
    let (remote, patches) = connect_losing_answer_of_patch(&harness, &owner, 2).await;
    let bytes = contents(4500);
    let source = source_of(&bytes);

    let node = remote
        .upload_chunked("flaky.bin", source.path(), None, CHUNK)
        .await
        .expect("uploading across a lost answer");

    assert_eq!(node.size, 4500);
    assert_eq!(
        &remote.read("flaky.bin").await.expect("reading")[..],
        &bytes[..]
    );
    assert_eq!(
        patches.load(Ordering::SeqCst),
        5,
        "five chunks, and the one whose answer was lost was asked about rather than resent"
    );
});

database_test!(
    an_upload_over_a_version_it_never_saw_is_refused_and_leaves_no_session,
    harness,
    {
        let owner = harness
            .account("stale-chunks@example.com", Role::Member)
            .await;
        let listed = harness.write(owner.id, "a.bin", b"listed").await;
        harness.write(owner.id, "a.bin", b"edited elsewhere").await;
        let remote = connect(&harness, &owner).await;
        let source = source_of(&contents(4500));

        let refused = remote
            .upload_chunked(
                "a.bin",
                source.path(),
                Some(&Expect::Etag(listed.etag.clone())),
                CHUNK,
            )
            .await;

        assert!(
            matches!(refused, Err(RemoteError::Changed(_))),
            "{refused:?}"
        );
        assert_eq!(harness.staged_uploads().await, 0);
        assert_eq!(
            &remote.read("a.bin").await.expect("reading")[..],
            b"edited elsewhere"
        );
    }
);

database_test!(
    a_file_that_cannot_fit_is_refused_before_it_is_sent,
    harness,
    {
        let owner = harness
            .account("full-chunks@example.com", Role::Member)
            .await;
        harness.root(owner.id).await;
        harness.set_quota(owner.id, 2000).await;
        let (remote, patches) = connect_losing_answer_of_patch(&harness, &owner, usize::MAX).await;
        let source = source_of(&contents(4500));

        let refused = remote
            .upload_chunked("big.bin", source.path(), None, CHUNK)
            .await;

        assert!(refused.is_err());
        assert_eq!(patches.load(Ordering::SeqCst), 0, "not one byte was sent");
    }
);
