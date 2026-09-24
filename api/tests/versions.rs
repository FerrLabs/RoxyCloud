mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use roxycloud_api::build_router;
use roxycloud_api::db;
use roxycloud_api::error::ApiError;
use roxycloud_core::blob::BlobHash;
use roxycloud_core::grant::Access;
use roxycloud_core::name::NodeName;
use roxycloud_core::node::Node;
use roxycloud_core::role::Role;
use serde_json::Value;
use tower::ServiceExt;

use common::Harness;

fn hash_of(contents: &[u8]) -> BlobHash {
    BlobHash::from(blake3::hash(contents))
}

async fn call(
    harness: &Harness,
    method: &str,
    path: &str,
    token: &str,
    body: &[u8],
) -> (StatusCode, Vec<u8>) {
    let response = build_router(harness.state.clone(), &[], None)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path.replace(' ', "%20"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
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
        .expect("reading the body")
        .to_bytes()
        .to_vec();
    (status, bytes)
}

fn ids(body: &[u8]) -> Vec<String> {
    serde_json::from_slice::<Vec<Value>>(body)
        .expect("a list of versions")
        .iter()
        .map(|version| version["id"].as_str().expect("an id").to_owned())
        .collect()
}

async fn version_count(harness: &Harness, owner: uuid::Uuid, path: &str) -> i64 {
    let node = harness.resolve(owner, path).await;
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM versions WHERE node_id = $1")
        .bind(node.id)
        .fetch_one(&harness.state.db)
        .await
        .expect("counting versions")
}

async fn holders(harness: &Harness, hash: BlobHash) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT (SELECT count(*) FROM nodes WHERE blob_hash = $1)
              + (SELECT count(*) FROM versions WHERE blob_hash = $1)",
    )
    .bind(hash)
    .fetch_one(&harness.state.db)
    .await
    .expect("counting what holds the blob")
}

database_test!(an_overwrite_keeps_what_it_replaced, harness, {
    let owner = harness.account("keep@example.com", Role::Member).await;
    let before = b"the draft that was right";

    harness.write(owner.id, "a.txt", before).await;
    harness
        .write(owner.id, "a.txt", b"the save that broke it")
        .await;

    assert_eq!(version_count(&harness, owner.id, "a.txt").await, 1);
    assert_eq!(
        harness.blob(hash_of(before)).await,
        Some((1, false)),
        "the version holds the replaced bytes, so the sweeper leaves them alone"
    );
});

database_test!(writing_the_same_bytes_again_keeps_no_version, harness, {
    let owner = harness.account("same@example.com", Role::Member).await;

    harness.write(owner.id, "a.txt", b"unchanged").await;
    harness.write(owner.id, "a.txt", b"unchanged").await;

    assert_eq!(
        version_count(&harness, owner.id, "a.txt").await,
        0,
        "nothing was replaced, so there is nothing to keep"
    );
});

database_test!(an_empty_file_is_replaced_without_a_version, harness, {
    let owner = harness.account("empty@example.com", Role::Member).await;

    harness.write(owner.id, "saved.docx", b"").await;
    harness
        .write(owner.id, "saved.docx", b"what the editor saved")
        .await;

    assert_eq!(
        version_count(&harness, owner.id, "saved.docx").await,
        0,
        "the empty file a LOCK leaves behind is nothing anyone would restore"
    );
});

async fn wait_for_waiters(harness: &Harness, count: i64) {
    for _ in 0..500 {
        let waiting = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM pg_stat_activity
             WHERE datname = current_database() AND wait_event_type = 'Lock'",
        )
        .fetch_one(&harness.state.db)
        .await
        .expect("reading pg_stat_activity");
        if waiting >= count {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("{count} writers never ended up waiting, so the race under test never happened");
}

async fn overwrite(harness: &Harness, root: &Node, (hash, size): (BlobHash, i64)) {
    let name = "a.txt".parse::<NodeName>().expect("a valid name");
    let mut tx = harness.state.db.begin().await.expect("begin");
    db::put_file(
        &mut tx,
        root.owner_id,
        root,
        &name,
        hash,
        size,
        harness.state.versions_kept,
    )
    .await
    .expect("the overwrite");
    tx.commit().await.expect("commit");
}

database_test!(
    two_overwrites_racing_keep_a_reference_for_every_version,
    harness,
    {
        let owner = harness.account("race@example.com", Role::Member).await;
        let original = b"the original";
        let first = b"the first rewrite";
        let second = b"the second rewrite";
        harness.write(owner.id, "a.txt", original).await;
        let root = harness.root(owner.id).await;
        let staged_first = harness.stage(first).await;
        let staged_second = harness.stage(second).await;

        let mut quota = harness.state.db.begin().await.expect("begin");
        sqlx::query("SELECT 1 FROM quotas WHERE owner_id = $1 FOR UPDATE")
            .bind(owner.id)
            .execute(&mut *quota)
            .await
            .expect("holding the quota row");

        tokio::join!(overwrite(&harness, &root, staged_first), async {
            wait_for_waiters(&harness, 1).await;
            tokio::join!(overwrite(&harness, &root, staged_second), async {
                wait_for_waiters(&harness, 2).await;
                quota.rollback().await.expect("rollback");
            });
        });

        assert_eq!(version_count(&harness, owner.id, "a.txt").await, 2);
        for contents in [&original[..], first, second] {
            assert_eq!(
                harness
                    .blob(hash_of(contents))
                    .await
                    .map(|(count, _)| count),
                Some(holders(&harness, hash_of(contents)).await),
                "every holder has its own reference, or pruning one frees bytes another still needs"
            );
        }
    }
);

database_test!(the_history_stops_at_the_cap, harness, {
    let owner = harness.account("cap@example.com", Role::Member).await;
    let oldest = b"version one";

    harness.write(owner.id, "a.txt", oldest).await;
    for contents in [
        b"version two",
        b"version 3!!",
        b"version 4!!",
        b"version 5!!",
    ] {
        harness.write(owner.id, "a.txt", contents).await;
    }

    assert_eq!(
        version_count(&harness, owner.id, "a.txt").await,
        harness.state.versions_kept,
        "the harness keeps three"
    );
    assert_eq!(
        harness.blob(hash_of(oldest)).await,
        Some((0, true)),
        "the version that fell off the end released its bytes"
    );
});

database_test!(
    turning_versions_off_drains_a_file_at_its_next_write,
    harness,
    {
        let owner = harness.account("drain@example.com", Role::Member).await;
        let kept = [b"version one", b"version two"];
        for contents in kept {
            harness.write(owner.id, "a.txt", contents).await;
        }
        harness.write(owner.id, "a.txt", b"version 3!!").await;
        assert_eq!(version_count(&harness, owner.id, "a.txt").await, 2);

        harness
            .write_without_versions(owner.id, "a.txt", b"version 4!!")
            .await;

        assert_eq!(version_count(&harness, owner.id, "a.txt").await, 0);
        assert_eq!(
            harness.used_bytes(owner.id).await,
            11,
            "only the current content is left to count"
        );
        for contents in kept {
            assert_eq!(harness.blob(hash_of(contents)).await, Some((0, true)));
        }
    }
);

database_test!(versions_count_against_the_quota, harness, {
    let owner = harness.account("count@example.com", Role::Member).await;

    harness.write(owner.id, "a.txt", &[b'x'; 400]).await;
    harness.write(owner.id, "a.txt", &[b'y'; 10]).await;

    assert_eq!(
        harness.used_bytes(owner.id).await,
        410,
        "the replaced 400 bytes are still stored, as a version"
    );
});

database_test!(
    a_write_that_would_not_fit_drops_the_oldest_history_first,
    harness,
    {
        let owner = harness.account("room@example.com", Role::Member).await;
        harness.write(owner.id, "a.txt", &[b'a'; 400]).await;
        harness.set_quota(owner.id, 1000).await;
        harness.write(owner.id, "a.txt", &[b'b'; 400]).await;
        assert_eq!(harness.used_bytes(owner.id).await, 800);

        harness.write(owner.id, "a.txt", &[b'c'; 400]).await;

        assert_eq!(
            harness.used_bytes(owner.id).await,
            800,
            "the oldest version made room, and the one just replaced was kept"
        );
        assert_eq!(version_count(&harness, owner.id, "a.txt").await, 1);
        assert_eq!(harness.blob(hash_of(&[b'a'; 400])).await, Some((0, true)));
        assert_eq!(harness.blob(hash_of(&[b'b'; 400])).await, Some((1, false)));
    }
);

database_test!(a_save_never_fails_because_history_is_kept, harness, {
    let owner = harness.account("never@example.com", Role::Member).await;
    harness.write(owner.id, "a.txt", &[b'a'; 600]).await;
    harness.set_quota(owner.id, 1000).await;

    harness.write(owner.id, "a.txt", &[b'b'; 600]).await;

    assert_eq!(
        harness.used_bytes(owner.id).await,
        600,
        "there was no room for a version, so the save went through without one"
    );
    assert_eq!(version_count(&harness, owner.id, "a.txt").await, 0);
});

database_test!(a_file_that_does_not_fit_still_fails_as_before, harness, {
    let owner = harness.account("toobig@example.com", Role::Member).await;
    harness.write(owner.id, "a.txt", &[b'a'; 600]).await;
    harness.set_quota(owner.id, 1000).await;

    let refused = harness.try_write(owner.id, "a.txt", &[b'b'; 1100]).await;

    assert!(matches!(refused, Err(ApiError::QuotaExceeded)));
    assert_eq!(harness.used_bytes(owner.id).await, 600);
});

database_test!(
    the_trash_takes_the_history_out_of_the_quota_and_back,
    harness,
    {
        let owner = harness.account("trash@example.com", Role::Member).await;
        harness.write(owner.id, "a.txt", &[b'a'; 400]).await;
        let file = harness.write(owner.id, "a.txt", &[b'b'; 10]).await;
        assert_eq!(harness.used_bytes(owner.id).await, 410);

        harness.trash(&file).await;
        assert_eq!(
            harness.used_bytes(owner.id).await,
            0,
            "a trashed file and its history stop counting together"
        );

        harness.restore(owner.id, file.id).await;
        assert_eq!(harness.used_bytes(owner.id).await, 410);
        assert_eq!(version_count(&harness, owner.id, "a.txt").await, 1);
    }
);

database_test!(purging_a_file_releases_its_history, harness, {
    let owner = harness.account("purge@example.com", Role::Member).await;
    let replaced = b"kept as a version";
    harness.write(owner.id, "a.txt", replaced).await;
    let file = harness.write(owner.id, "a.txt", b"current").await;

    harness.trash(&file).await;
    harness.purge(owner.id, file.id).await;

    assert_eq!(
        harness.blob(hash_of(replaced)).await,
        Some((0, true)),
        "no row can reach the version any more, so its bytes must be sweepable"
    );
});

database_test!(versions_are_listed_downloaded_and_restored, harness, {
    let owner = harness.account("http@example.com", Role::Member).await;
    let token = harness.state.sessions.issue(owner.id).expect("a token");

    call(&harness, "PUT", "/v1/files/notes.md", &token, b"first").await;
    call(&harness, "PUT", "/v1/files/notes.md", &token, b"second").await;

    let (status, body) = call(&harness, "GET", "/v1/versions/notes.md", &token, b"").await;
    assert_eq!(status, StatusCode::OK);
    let listed = ids(&body);
    assert_eq!(listed.len(), 1);

    let (status, body) = call(
        &harness,
        "GET",
        &format!("/v1/version/{}/notes.md", listed[0]),
        &token,
        b"",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"first");

    let (status, _) = call(
        &harness,
        "POST",
        &format!("/v1/version/{}/notes.md", listed[0]),
        &token,
        b"",
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, current) = call(&harness, "GET", "/v1/files/notes.md", &token, b"").await;
    assert_eq!(current, b"first", "the restore wrote the old content back");

    let (_, body) = call(&harness, "GET", "/v1/versions/notes.md", &token, b"").await;
    let history = ids(&body);
    assert_eq!(
        history.len(),
        1,
        "the restored version is the file again, so it left the history"
    );
    let (_, replaced) = call(
        &harness,
        "GET",
        &format!("/v1/version/{}/notes.md", history[0]),
        &token,
        b"",
    )
    .await;
    assert_eq!(
        replaced, b"second",
        "what the restore replaced became a version, so the restore is undoable"
    );
});

database_test!(a_restore_does_not_count_the_same_bytes_twice, harness, {
    let owner = harness.account("twice@example.com", Role::Member).await;
    let token = harness.state.sessions.issue(owner.id).expect("a token");
    call(&harness, "PUT", "/v1/files/a.txt", &token, &[b'a'; 300]).await;
    call(&harness, "PUT", "/v1/files/a.txt", &token, &[b'b'; 100]).await;
    let (_, body) = call(&harness, "GET", "/v1/versions/a.txt", &token, b"").await;
    let version = ids(&body).remove(0);

    let (status, _) = call(
        &harness,
        "POST",
        &format!("/v1/version/{version}/a.txt"),
        &token,
        b"",
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    assert_eq!(
        harness.used_bytes(owner.id).await,
        400,
        "300 bytes back in the file and 100 kept as a version, nothing counted twice"
    );
    assert_eq!(
        harness.blob(hash_of(&[b'a'; 300])).await,
        Some((1, false)),
        "only the file holds the restored bytes now"
    );
});

database_test!(a_version_is_only_reached_through_its_own_file, harness, {
    let owner = harness.account("own@example.com", Role::Member).await;
    let token = harness.state.sessions.issue(owner.id).expect("a token");

    call(
        &harness,
        "PUT",
        "/v1/files/secret.txt",
        &token,
        b"old secret",
    )
    .await;
    call(
        &harness,
        "PUT",
        "/v1/files/secret.txt",
        &token,
        b"new secret",
    )
    .await;
    call(&harness, "PUT", "/v1/files/public.txt", &token, b"anything").await;
    let (_, body) = call(&harness, "GET", "/v1/versions/secret.txt", &token, b"").await;
    let secret = ids(&body).remove(0);

    let (status, _) = call(
        &harness,
        "GET",
        &format!("/v1/version/{secret}/public.txt"),
        &token,
        b"",
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a readable path must not unlock another file's version"
    );
});

database_test!(
    a_reader_of_a_share_sees_the_history_but_cannot_restore_it,
    harness,
    {
        let owner = harness.account("owner@example.com", Role::Member).await;
        let guest = harness.account("guest@example.com", Role::Member).await;
        harness.write(owner.id, "photos/list.txt", b"one").await;
        harness.write(owner.id, "photos/list.txt", b"two").await;
        harness
            .grant(&owner, "photos", "guest@example.com", Access::Read)
            .await;
        let token = harness.state.sessions.issue(guest.id).expect("a token");
        let shared = "Shared with me/photos/list.txt";

        let (status, body) = call(
            &harness,
            "GET",
            &format!("/v1/versions/{shared}"),
            &token,
            b"",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let version = ids(&body).remove(0);

        let (status, body) = call(
            &harness,
            "GET",
            &format!("/v1/version/{version}/{shared}"),
            &token,
            b"",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"one");

        let (status, _) = call(
            &harness,
            "POST",
            &format!("/v1/version/{version}/{shared}"),
            &token,
            b"",
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
);

database_test!(a_stranger_cannot_see_the_history, harness, {
    let owner = harness.account("mine@example.com", Role::Member).await;
    let stranger = harness.account("stranger@example.com", Role::Member).await;
    harness.write(owner.id, "a.txt", b"one").await;
    harness.write(owner.id, "a.txt", b"two").await;
    let token = harness.state.sessions.issue(stranger.id).expect("a token");

    let (status, _) = call(&harness, "GET", "/v1/versions/a.txt", &token, b"").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the path resolves in the stranger's own tree, where there is no such file"
    );
});
