mod common;

use std::time::Duration;

use roxycloud_api::sweeper::{Collected, sweep};
use roxycloud_core::blob::BlobHash;
use roxycloud_core::role::Role;

fn hash_of(contents: &[u8]) -> BlobHash {
    BlobHash::from(blake3::hash(contents))
}

const HOUR: Duration = Duration::from_secs(60 * 60);

database_test!(a_blob_nothing_points_at_is_collected, harness, {
    let owner = harness.account("sweep@example.com", Role::Member).await;
    let contents = b"uploaded, deleted, purged";
    let node = harness.write(owner.id, "gone.txt", contents).await;
    harness.trash(&node).await;
    harness.purge(owner.id, node.id).await;
    assert!(harness.blob_file_exists(hash_of(contents)).await);

    let collected = sweep(&harness.state, Duration::ZERO)
        .await
        .expect("sweeping");

    assert_eq!(
        collected,
        Collected {
            blobs: 1,
            bytes: i64::try_from(contents.len()).expect("small test payload"),
        }
    );
    assert_eq!(
        harness.blob(hash_of(contents)).await,
        None,
        "the row is gone"
    );
    assert!(
        !harness.blob_file_exists(hash_of(contents)).await,
        "and so are the bytes, which is the point of the whole thing"
    );
});

database_test!(a_blob_that_regained_a_reference_is_left_alone, harness, {
    let owner = harness.account("regained@example.com", Role::Member).await;
    let contents = b"deleted, then uploaded again";
    let node = harness.write(owner.id, "first.txt", contents).await;
    harness.trash(&node).await;
    harness.purge(owner.id, node.id).await;
    assert_eq!(harness.blob(hash_of(contents)).await, Some((0, true)));

    harness.write(owner.id, "second.txt", contents).await;
    let collected = sweep(&harness.state, Duration::ZERO)
        .await
        .expect("sweeping");

    assert_eq!(collected, Collected::default(), "nothing was collectable");
    assert_eq!(
        harness.blob(hash_of(contents)).await,
        Some((1, false)),
        "the re-upload adopted the blob, and the grace period is what gives it the chance"
    );
    assert!(harness.blob_file_exists(hash_of(contents)).await);
});

database_test!(
    a_blob_still_inside_its_grace_period_is_left_alone,
    harness,
    {
        let owner = harness.account("grace@example.com", Role::Member).await;
        let contents = b"only just released";
        let node = harness.write(owner.id, "gone.txt", contents).await;
        harness.trash(&node).await;
        harness.purge(owner.id, node.id).await;

        let collected = sweep(&harness.state, HOUR).await.expect("sweeping");

        assert_eq!(collected, Collected::default());
        assert_eq!(harness.blob(hash_of(contents)).await, Some((0, true)));
        assert!(harness.blob_file_exists(hash_of(contents)).await);
    }
);

database_test!(a_blob_a_trashed_file_still_holds_is_left_alone, harness, {
    let owner = harness.account("trashheld@example.com", Role::Member).await;
    let contents = b"in the trash, not gone";
    let node = harness.write(owner.id, "waiting.txt", contents).await;
    harness.trash(&node).await;

    let collected = sweep(&harness.state, Duration::ZERO)
        .await
        .expect("sweeping");

    assert_eq!(collected, Collected::default());
    assert!(
        harness.blob_file_exists(hash_of(contents)).await,
        "collecting this would make the restore in the trash a lie"
    );
    harness.restore(owner.id, node.id).await;
    assert_eq!(harness.blob(hash_of(contents)).await, Some((1, false)));
});

database_test!(bytes_written_since_the_row_went_stale_are_kept, harness, {
    let owner = harness.account("rewritten@example.com", Role::Member).await;
    let contents = b"written again while the sweeper was deciding";
    let node = harness.write(owner.id, "gone.txt", contents).await;
    harness.trash(&node).await;
    harness.purge(owner.id, node.id).await;
    harness.age_blob(hash_of(contents), 2 * HOUR).await;

    let collected = sweep(&harness.state, HOUR).await.expect("sweeping");

    assert_eq!(
        collected,
        Collected::default(),
        "the row was stale but the file is not, so something wrote those bytes since"
    );
    assert_eq!(
        harness.blob(hash_of(contents)).await,
        None,
        "the stale row still goes, an upload that raced us writes its own"
    );
    assert!(
        harness.blob_file_exists(hash_of(contents)).await,
        "deleting bytes an upload just wrote would leave its node pointing at nothing"
    );
});

database_test!(a_blob_referenced_again_mid_sweep_survives, harness, {
    let owner = harness.account("midsweep@example.com", Role::Member).await;
    let contents = b"claimed between the read and the delete";
    let node = harness.write(owner.id, "gone.txt", contents).await;
    harness.trash(&node).await;
    harness.purge(owner.id, node.id).await;

    let mut racer = harness.state.db.begin().await.expect("begin");
    sqlx::query("UPDATE blobs SET ref_count = 1, unreferenced_since = NULL WHERE hash = $1")
        .bind(hash_of(contents))
        .execute(&mut *racer)
        .await
        .expect("an upload adopting the blob");

    let (collected, ()) = tokio::join!(sweep(&harness.state, Duration::ZERO), async {
        harness.wait_until_blocked_on("DELETE FROM blobs").await;
        racer.commit().await.expect("commit");
    });

    assert_eq!(
        collected.expect("sweeping"),
        Collected::default(),
        "the read said nothing pointed at it, the delete has to check again"
    );
    assert_eq!(harness.blob(hash_of(contents)).await, Some((1, false)));
    assert!(harness.blob_file_exists(hash_of(contents)).await);
});
