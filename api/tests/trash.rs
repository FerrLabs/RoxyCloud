mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::{DateTime, TimeDelta, Utc};
use http_body_util::BodyExt;
use roxycloud_api::build_router;
use roxycloud_api::state::AppState;
use roxycloud_api::trash;
use roxycloud_core::blob::BlobHash;
use roxycloud_core::role::Role;
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;

use common::Harness;

fn hash_of(contents: &[u8]) -> BlobHash {
    BlobHash::from(blake3::hash(contents))
}

async fn call(state: &AppState, method: &str, path: &str, token: &str) -> (StatusCode, Vec<u8>) {
    let response = build_router(state.clone(), &[], None)
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
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

async fn empty(harness: &Harness, owner: Uuid) -> u64 {
    let mut tx = harness.state.db.begin().await.expect("begin");
    let purged = trash::empty(&mut tx, owner).await.expect("emptying");
    tx.commit().await.expect("commit");
    purged
}

async fn age(harness: &Harness, owner: Uuid, name: &str, by: TimeDelta) {
    let id = harness.resolve_trashed(owner, name).await;
    sqlx::query("UPDATE nodes SET deleted_at = deleted_at - $2 WHERE trash_root_id = $1")
        .bind(id)
        .bind(by)
        .execute(&harness.state.db)
        .await
        .expect("ageing the trash entry");
}

database_test!(
    emptying_the_trash_purges_all_of_it_and_nothing_else,
    harness,
    {
        let owner = harness.account("empty@example.com", Role::Member).await;
        let neighbour = harness.account("neighbour@example.com", Role::Member).await;
        let file = harness.write(owner.id, "a.txt", b"deleted file").await;
        harness
            .write(owner.id, "album/b.txt", b"deleted with its folder")
            .await;
        let album = harness.resolve(owner.id, "album").await;
        harness.write(owner.id, "keep.txt", b"never deleted").await;
        let theirs = harness
            .write(neighbour.id, "x.txt", b"another account")
            .await;
        harness.trash(&file).await;
        harness.trash(&album).await;
        harness.trash(&theirs).await;

        assert_eq!(empty(&harness, owner.id).await, 2);

        assert!(harness.trashed(owner.id).await.is_empty());
        assert_eq!(harness.trashed(neighbour.id).await, ["x.txt"]);
        assert_eq!(harness.resolve(owner.id, "keep.txt").await.name, "keep.txt");
        for contents in [&b"deleted file"[..], b"deleted with its folder"] {
            assert_eq!(harness.blob(hash_of(contents)).await, Some((0, true)));
        }
        assert_eq!(
            harness.blob(hash_of(b"another account")).await,
            Some((1, false)),
            "the trash of another account is not this one to empty"
        );
    }
);

database_test!(
    a_folder_and_what_was_deleted_from_it_earlier_both_go,
    harness,
    {
        let owner = harness.account("nested@example.com", Role::Member).await;
        let inner = harness
            .write(owner.id, "album/first.txt", b"deleted first")
            .await;
        harness
            .write(owner.id, "album/second.txt", b"deleted with the folder")
            .await;
        let album = harness.resolve(owner.id, "album").await;
        harness.trash(&inner).await;
        harness.trash(&album).await;
        assert_eq!(harness.trashed(owner.id).await.len(), 2);

        empty(&harness, owner.id).await;

        assert!(harness.trashed(owner.id).await.is_empty());
        for contents in [&b"deleted first"[..], b"deleted with the folder"] {
            assert_eq!(harness.blob(hash_of(contents)).await, Some((0, true)));
        }
    }
);

database_test!(
    what_outlived_the_retention_is_purged_and_the_rest_kept,
    harness,
    {
        let owner = harness.account("expire@example.com", Role::Member).await;
        let old = harness
            .write(owner.id, "old.txt", b"deleted long ago")
            .await;
        let recent = harness
            .write(owner.id, "recent.txt", b"deleted today")
            .await;
        harness.trash(&old).await;
        harness.trash(&recent).await;
        age(&harness, owner.id, "old.txt", TimeDelta::days(31)).await;

        let purged = trash::expire(&harness.state.db, TimeDelta::days(30))
            .await
            .expect("expiring");

        assert_eq!(purged, 1);
        assert_eq!(harness.trashed(owner.id).await, ["recent.txt"]);
        assert_eq!(
            harness.blob(hash_of(b"deleted long ago")).await,
            Some((0, true))
        );
        assert_eq!(
            harness.blob(hash_of(b"deleted today")).await,
            Some((1, false))
        );
    }
);

database_test!(
    an_expired_folder_takes_an_expired_entry_inside_it_along,
    harness,
    {
        let owner = harness
            .account("expirenested@example.com", Role::Member)
            .await;
        let inner = harness
            .write(owner.id, "album/first.txt", b"deleted first")
            .await;
        harness
            .write(owner.id, "album/second.txt", b"deleted with the folder")
            .await;
        let album = harness.resolve(owner.id, "album").await;
        harness.trash(&inner).await;
        harness.trash(&album).await;
        age(&harness, owner.id, "album", TimeDelta::days(35)).await;
        age(&harness, owner.id, "first.txt", TimeDelta::days(40)).await;

        trash::expire(&harness.state.db, TimeDelta::days(30))
            .await
            .expect("an entry already purged with its folder is skipped, not an error");

        assert!(harness.trashed(owner.id).await.is_empty());
    }
);

database_test!(
    a_root_that_cannot_be_purged_does_not_hold_up_the_rest,
    harness,
    {
        let owner = harness.account("stuck@example.com", Role::Member).await;
        let older = harness
            .write(owner.id, "older.txt", b"expired, behind the stuck one")
            .await;
        let stuck = harness
            .write(owner.id, "stuck.txt", b"expired, cannot go")
            .await;
        harness.trash(&older).await;
        harness.trash(&stuck).await;
        age(&harness, owner.id, "older.txt", TimeDelta::days(40)).await;
        age(&harness, owner.id, "stuck.txt", TimeDelta::days(35)).await;
        for statement in [
            "CREATE FUNCTION refuse_stuck() RETURNS trigger AS $$
             BEGIN
                 IF OLD.name = 'stuck.txt' THEN RAISE EXCEPTION 'stuck'; END IF;
                 RETURN OLD;
             END $$ LANGUAGE plpgsql",
            "CREATE TRIGGER refuse_stuck BEFORE DELETE ON nodes
             FOR EACH ROW EXECUTE FUNCTION refuse_stuck()",
        ] {
            sqlx::query(statement)
                .execute(&harness.state.db)
                .await
                .expect("installing the trigger");
        }

        let purged = trash::expire(&harness.state.db, TimeDelta::days(30))
            .await
            .expect("a root that fails is logged and passed over, not the end of the run");

        assert_eq!(purged, 1);
        assert_eq!(harness.trashed(owner.id).await, ["stuck.txt"]);
    }
);

database_test!(the_trash_is_emptied_over_http_by_a_writer_only, harness, {
    let member = harness.account("writer@example.com", Role::Member).await;
    let reader = harness.account("reader@example.com", Role::Reader).await;
    let file = harness.write(member.id, "a.txt", b"to be emptied").await;
    harness.trash(&file).await;
    let member_token = harness.state.sessions.issue(member.id).expect("a token");
    let reader_token = harness.state.sessions.issue(reader.id).expect("a token");

    let (status, _) = call(&harness.state, "DELETE", "/v1/trash", &reader_token).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = call(&harness.state, "DELETE", "/v1/trash", &member_token).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = call(&harness.state, "GET", "/v1/trash", &member_token).await;
    assert_eq!(body, b"[]");
});

database_test!(
    the_listing_says_when_each_entry_goes_once_a_retention_is_set,
    harness,
    {
        let owner = harness.account("expiry@example.com", Role::Member).await;
        let file = harness.write(owner.id, "a.txt", b"on a clock").await;
        harness.trash(&file).await;
        let token = harness.state.sessions.issue(owner.id).expect("a token");

        let (_, body) = call(&harness.state, "GET", "/v1/trash", &token).await;
        let listed: Vec<Value> = serde_json::from_slice(&body).expect("a list");
        assert!(
            listed[0].get("expires_at").is_none(),
            "no retention, so nothing expires"
        );

        let mut kept_a_month = harness.state.clone();
        kept_a_month.trash_retention = Some(TimeDelta::days(30));
        let (_, body) = call(&kept_a_month, "GET", "/v1/trash", &token).await;
        let listed: Vec<Value> = serde_json::from_slice(&body).expect("a list");
        let at = |field: &str| {
            listed[0][field]
                .as_str()
                .and_then(|text| text.parse::<DateTime<Utc>>().ok())
                .expect("a timestamp")
        };
        assert_eq!(at("expires_at") - at("deleted_at"), TimeDelta::days(30));
        assert_eq!(
            listed[0]["name"], "a.txt",
            "the node keeps its own fields beside the expiry"
        );
    }
);
