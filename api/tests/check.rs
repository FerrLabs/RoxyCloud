mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use stashden_api::check::{Options, run};
use stashden_api::{build_router, db};
use stashden_core::blob::BlobHash;
use stashden_core::name::NodeName;
use stashden_core::role::Role;
use tower::ServiceExt;
use uuid::Uuid;

use common::Harness;

async fn check(harness: &Harness, options: Options) -> stashden_api::check::Findings {
    run(&harness.state.db, harness.state.blobs.as_ref(), options)
        .await
        .expect("checking")
}

const CONTENT: Options = Options {
    verify_content: true,
    repair: false,
};

const REPAIR: Options = Options {
    verify_content: true,
    repair: true,
};

fn png() -> Vec<u8> {
    let image = image::RgbImage::from_fn(300, 200, |x, y| {
        image::Rgb([
            u8::try_from(x % 256).unwrap_or(0),
            u8::try_from(y % 256).unwrap_or(0),
            90,
        ])
    });
    let mut encoded = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, image::ImageFormat::Png)
        .expect("encoding the fixture");
    encoded.into_inner()
}

async fn thumbnail_of(harness: &Harness, owner: Uuid, path: &str) {
    let token = harness.session(owner).await;
    let response = build_router(harness.state.clone(), &[], None)
        .oneshot(
            Request::builder()
                .uri(format!("/v1/thumbnails/{path}?edge=128"))
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .expect("a well formed request"),
        )
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::OK);
}

async fn copy_of(harness: &Harness, owner: Uuid, from: &str, to: &str) {
    let source = harness.resolve(owner, from).await;
    let root = harness.root(owner).await;
    let name: NodeName = to.parse().expect("a name");
    let mut tx = harness.state.db.begin().await.expect("begin");
    db::copy_tree(&mut tx, owner, &source, &root, &name)
        .await
        .expect("copying");
    tx.commit().await.expect("commit");
}

async fn lived_in(harness: &Harness) -> (Uuid, BlobHash) {
    let owner = harness.account("keeper@example.com", Role::Member).await;
    let first = harness
        .write(owner.id, "docs/plan.md", b"the first draft")
        .await;
    harness
        .write(owner.id, "docs/plan.md", b"the second draft")
        .await;
    harness
        .write(owner.id, "docs/plan.md", b"the third draft")
        .await;
    harness
        .write(owner.id, "photos/beach.jpg", b"sand and sea")
        .await;
    harness
        .write(owner.id, "photos/copy.jpg", b"sand and sea")
        .await;
    let gone = harness.write(owner.id, "scratch.txt", b"throw away").await;
    let kept = harness
        .write(owner.id, "moved/a.txt", b"will be moved")
        .await;
    harness.rename(owner.id, "moved/a.txt", "moved/b.txt").await;
    harness.trash(&gone).await;
    let trashed = harness.resolve_trashed(owner.id, "scratch.txt").await;
    harness.restore(owner.id, trashed).await;
    let doomed = harness
        .write(owner.id, "doomed.txt", b"purged for good")
        .await;
    harness.trash(&doomed).await;
    let doomed_id = harness.resolve_trashed(owner.id, "doomed.txt").await;
    harness.purge(owner.id, doomed_id).await;
    harness.write(owner.id, "pic.png", &png()).await;
    thumbnail_of(harness, owner.id, "pic.png").await;
    copy_of(harness, owner.id, "docs", "docs-copy").await;
    let _ = first;
    (owner.id, kept.blob_hash.expect("a file"))
}

database_test!(an_instance_that_has_been_used_is_clean, harness, {
    lived_in(&harness).await;

    let findings = check(&harness, CONTENT).await;

    assert!(findings.is_clean(), "{findings:?}");
    assert!(findings.checked_blobs >= 5, "{}", findings.checked_blobs);
});

database_test!(a_blob_that_is_gone_from_the_store_is_reported, harness, {
    let (_, hash) = lived_in(&harness).await;
    harness.state.blobs.remove(hash).await.expect("removing");

    let findings = check(&harness, Options::default()).await;

    assert_eq!(findings.missing, [hash]);
    assert!(findings.needs_a_person());
});

database_test!(
    an_unreferenced_blob_missing_from_the_store_is_not_a_problem,
    harness,
    {
        let owner = harness.account("sweeper@example.com", Role::Member).await;
        let node = harness
            .write(owner.id, "gone.txt", b"collected already")
            .await;
        let hash = node.blob_hash.expect("a file");
        harness.trash(&node).await;
        let id = harness.resolve_trashed(owner.id, "gone.txt").await;
        harness.purge(owner.id, id).await;
        harness.state.blobs.remove(hash).await.expect("removing");

        let findings = check(&harness, CONTENT).await;

        assert!(findings.is_clean(), "{findings:?}");
    }
);

database_test!(
    bytes_that_no_longer_match_their_hash_are_reported_only_when_asked,
    harness,
    {
        let (_, hash) = lived_in(&harness).await;
        std::fs::write(harness.blob_file(hash), b"bit rot").expect("damaging the file");

        let quick = check(&harness, Options::default()).await;
        let thorough = check(&harness, CONTENT).await;

        assert!(quick.is_clean(), "presence alone cannot tell: {quick:?}");
        assert_eq!(thorough.corrupt, [hash]);
    }
);

database_test!(a_wrong_reference_count_is_reported_and_repaired, harness, {
    let (_, hash) = lived_in(&harness).await;
    sqlx::query("UPDATE blobs SET ref_count = ref_count + 4 WHERE hash = $1")
        .bind(hash)
        .execute(&harness.state.db)
        .await
        .expect("tampering");

    let reported = check(&harness, CONTENT).await;
    let repaired = check(&harness, REPAIR).await;
    let after = check(&harness, CONTENT).await;

    assert_eq!(reported.wrong_counts.len(), 1, "{reported:?}");
    assert_eq!(
        reported.wrong_counts[0].stored - reported.wrong_counts[0].expected,
        4
    );
    assert!(repaired.repaired);
    assert!(after.is_clean(), "{after:?}");
});

database_test!(
    a_count_that_fell_to_zero_is_repaired_so_the_sweep_cannot_collect_it,
    harness,
    {
        let (_, hash) = lived_in(&harness).await;
        sqlx::query("UPDATE blobs SET ref_count = 0, unreferenced_since = now() WHERE hash = $1")
            .bind(hash)
            .execute(&harness.state.db)
            .await
            .expect("tampering");

        check(&harness, REPAIR).await;

        assert_eq!(
            harness.blob(hash).await,
            Some((1, false)),
            "referenced again, and no longer marked for collection"
        );
    }
);

database_test!(a_quota_that_drifted_is_reported_and_repaired, harness, {
    let (owner, _) = lived_in(&harness).await;
    let honest = harness.used_bytes(owner).await;
    sqlx::query("UPDATE quotas SET bytes_used = 3 WHERE owner_id = $1")
        .bind(owner)
        .execute(&harness.state.db)
        .await
        .expect("tampering");

    let reported = check(&harness, Options::default()).await;
    check(
        &harness,
        Options {
            verify_content: false,
            repair: true,
        },
    )
    .await;

    assert_eq!(reported.wrong_usage.len(), 1, "{reported:?}");
    assert_eq!(reported.wrong_usage[0].expected, honest);
    assert_eq!(harness.used_bytes(owner).await, honest);
});

database_test!(
    repairing_does_not_pretend_a_missing_blob_is_fixed,
    harness,
    {
        let (_, hash) = lived_in(&harness).await;
        harness.state.blobs.remove(hash).await.expect("removing");

        let findings = check(&harness, REPAIR).await;

        assert_eq!(findings.missing, [hash]);
        assert!(findings.needs_a_person());
    }
);
