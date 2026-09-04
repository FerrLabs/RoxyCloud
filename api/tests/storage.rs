mod common;

use roxycloud_api::error::ApiError;
use roxycloud_core::blob::BlobHash;
use roxycloud_core::role::Role;

fn hash_of(contents: &[u8]) -> BlobHash {
    BlobHash::from(blake3::hash(contents))
}

database_test!(the_root_is_the_one_node_without_a_name, harness, {
    let owner = harness.account("root@example.com", Role::Member).await;

    let first = harness.root(owner.id).await;
    let second = harness.root(owner.id).await;

    assert_eq!(first.id, second.id, "ensure_root is idempotent");
    assert_eq!(first.name, "", "the root carries no name");
    assert!(first.parent_id.is_none());
});

database_test!(identical_bytes_are_stored_once, harness, {
    let owner = harness.account("dedup@example.com", Role::Member).await;
    let shared = b"the same bytes in two places";

    harness.write(owner.id, "one.txt", shared).await;
    harness.write(owner.id, "nested/two.txt", shared).await;

    assert_eq!(
        harness.blob(hash_of(shared)).await,
        Some((2, false)),
        "one blob, referenced twice"
    );
});

database_test!(
    a_blob_becomes_collectable_only_when_nothing_points_at_it,
    harness,
    {
        let owner = harness.account("refcount@example.com", Role::Member).await;
        let shared = b"referenced twice, then not at all";

        let first = harness.write(owner.id, "one.txt", shared).await;
        let second = harness.write(owner.id, "two.txt", shared).await;

        harness.trash(&first).await;
        assert_eq!(
            harness.blob(hash_of(shared)).await,
            Some((1, false)),
            "the surviving file still holds the bytes"
        );

        harness.trash(&second).await;
        assert_eq!(
            harness.blob(hash_of(shared)).await,
            Some((0, true)),
            "the last reference marks the blob for the sweeper"
        );
    }
);

database_test!(an_overwrite_credits_the_size_it_replaces, harness, {
    let owner = harness.account("overwrite@example.com", Role::Member).await;

    harness.write(owner.id, "a.txt", &[b'x'; 400]).await;
    assert_eq!(harness.used_bytes(owner.id).await, 400);

    harness.write(owner.id, "a.txt", &[b'y'; 10]).await;
    assert_eq!(
        harness.used_bytes(owner.id).await,
        10,
        "the quota holds what the file weighs now, not the sum of what it ever weighed"
    );
});

database_test!(an_overwrite_releases_the_bytes_it_replaces, harness, {
    let owner = harness.account("release@example.com", Role::Member).await;
    let before = b"the first contents";
    let after = b"the second contents";

    harness.write(owner.id, "a.txt", before).await;
    harness.write(owner.id, "a.txt", after).await;

    assert_eq!(
        harness.blob(hash_of(before)).await,
        Some((0, true)),
        "the replaced bytes are no longer referenced"
    );
    assert_eq!(harness.blob(hash_of(after)).await, Some((1, false)));
});

database_test!(rewriting_the_same_bytes_keeps_one_reference, harness, {
    let owner = harness.account("rewrite@example.com", Role::Member).await;
    let same = b"unchanged between two uploads";

    harness.write(owner.id, "a.txt", same).await;
    harness.write(owner.id, "a.txt", same).await;

    assert_eq!(
        harness.blob(hash_of(same)).await,
        Some((1, false)),
        "acquiring then releasing the same blob nets out"
    );
    assert_eq!(
        harness.used_bytes(owner.id).await,
        i64::try_from(same.len()).expect("small test payload")
    );
});

database_test!(a_write_past_the_quota_changes_nothing, harness, {
    let owner = harness.account("quota@example.com", Role::Member).await;
    harness.root(owner.id).await;
    harness.set_quota(owner.id, 100).await;

    harness.write(owner.id, "small.txt", &[b'x'; 60]).await;
    let refused = harness.try_write(owner.id, "big.txt", &[b'y'; 60]).await;

    assert!(
        matches!(refused, Err(ApiError::QuotaExceeded)),
        "the second write does not fit"
    );
    assert_eq!(
        harness.used_bytes(owner.id).await,
        60,
        "the refused write left the quota alone"
    );
    let root = harness.root(owner.id).await;
    assert_eq!(harness.children(&root).await, ["small.txt"]);
});

database_test!(a_file_cannot_take_the_name_of_a_directory, harness, {
    let owner = harness.account("kind@example.com", Role::Member).await;
    harness.write(owner.id, "photos/x.txt", b"inside").await;

    let clash = harness
        .try_write(owner.id, "photos", b"on top of the folder")
        .await;

    assert!(matches!(clash, Err(ApiError::Conflict(_))), "got {clash:?}");
});

database_test!(a_name_belongs_to_one_node_per_directory, harness, {
    let owner = harness.account("unique@example.com", Role::Member).await;

    let first = harness.write(owner.id, "a.txt", b"first").await;
    let second = harness.write(owner.id, "a.txt", b"second").await;

    assert_eq!(
        first.id, second.id,
        "the second write updates the first node"
    );
    let root = harness.root(owner.id).await;
    assert_eq!(harness.children(&root).await, ["a.txt"]);
});

database_test!(a_trashed_node_leaves_the_listing, harness, {
    let owner = harness.account("listing@example.com", Role::Member).await;
    harness.write(owner.id, "kept.txt", b"kept").await;
    let doomed = harness.write(owner.id, "gone.txt", b"gone").await;

    harness.trash(&doomed).await;

    let root = harness.root(owner.id).await;
    assert_eq!(harness.children(&root).await, ["kept.txt"]);
});

database_test!(
    trashing_the_same_node_twice_leaves_the_quota_honest,
    harness,
    {
        let owner = harness.account("double@example.com", Role::Member).await;
        let node = harness.write(owner.id, "a.txt", b"written once").await;

        harness.trash(&node).await;
        harness.trash(&node).await;

        assert_eq!(
            harness.used_bytes(owner.id).await,
            0,
            "a second trash is a no-op, not a second refund"
        );
        assert_eq!(
            harness.blob(hash_of(b"written once")).await,
            Some((0, true))
        );
    }
);

database_test!(a_rename_keeps_the_node_and_its_bytes, harness, {
    let owner = harness.account("rename@example.com", Role::Member).await;
    let before = harness
        .write(owner.id, "draft.txt", b"the same bytes")
        .await;

    let after = harness.rename(owner.id, "draft.txt", "final.txt").await;

    assert_eq!(after.id, before.id, "renaming does not create a new node");
    assert_eq!(
        after.etag, before.etag,
        "a file's etag follows its content, and the content did not change"
    );
    assert!(after.updated_at > before.updated_at);
    let root = harness.root(owner.id).await;
    assert_eq!(harness.children(&root).await, ["final.txt"]);
});

database_test!(a_move_carries_the_subtree_with_it, harness, {
    let owner = harness.account("subtree@example.com", Role::Member).await;
    harness.write(owner.id, "inbox/notes/a.txt", b"deep").await;
    harness
        .write(owner.id, "archive/keep.txt", b"already there")
        .await;

    harness
        .rename(owner.id, "inbox/notes", "archive/notes")
        .await;

    let moved = harness.resolve(owner.id, "archive/notes").await;
    assert_eq!(
        harness.children(&moved).await,
        ["a.txt"],
        "the child came along"
    );
    let inbox = harness.resolve(owner.id, "inbox").await;
    assert!(harness.children(&inbox).await.is_empty());
});

database_test!(a_directory_cannot_be_moved_inside_itself, harness, {
    let owner = harness.account("cycle@example.com", Role::Member).await;
    harness
        .write(owner.id, "photos/summer/x.txt", b"deep")
        .await;

    let refused = harness
        .try_rename(owner.id, "photos", "photos/summer/photos")
        .await;

    assert!(
        matches!(refused, Err(ApiError::MoveIntoSelf)),
        "got {refused:?}"
    );
    assert_eq!(
        harness
            .children(&harness.resolve(owner.id, "photos/summer").await)
            .await,
        ["x.txt"],
        "the subtree is untouched"
    );
});

database_test!(a_directory_cannot_be_moved_into_itself_directly, harness, {
    let owner = harness.account("selfmove@example.com", Role::Member).await;
    harness.write(owner.id, "photos/x.txt", b"inside").await;

    let refused = harness
        .try_rename(owner.id, "photos", "photos/photos")
        .await;

    assert!(
        matches!(refused, Err(ApiError::MoveIntoSelf)),
        "got {refused:?}"
    );
});

database_test!(
    a_move_onto_an_occupied_name_conflicts_instead_of_overwriting,
    harness,
    {
        let owner = harness.account("occupied@example.com", Role::Member).await;
        let kept = harness
            .write(owner.id, "a.txt", b"the one that stays")
            .await;
        harness
            .write(owner.id, "b.txt", b"the one that moves")
            .await;

        let refused = harness.try_rename(owner.id, "b.txt", "a.txt").await;

        assert!(
            matches!(refused, Err(ApiError::Conflict(_))),
            "got {refused:?}"
        );
        assert_eq!(
            harness.resolve(owner.id, "a.txt").await.blob_hash,
            kept.blob_hash,
            "the occupant still holds its own bytes"
        );
        let root = harness.root(owner.id).await;
        assert_eq!(harness.children(&root).await, ["a.txt", "b.txt"]);
    }
);

database_test!(a_move_leaves_the_quota_and_the_blob_alone, harness, {
    let owner = harness.account("movequota@example.com", Role::Member).await;
    let contents = b"weighed once, moved twice";
    harness.write(owner.id, "a.txt", contents).await;
    let used = harness.used_bytes(owner.id).await;

    harness.rename(owner.id, "a.txt", "elsewhere.txt").await;

    assert_eq!(harness.used_bytes(owner.id).await, used);
    assert_eq!(harness.blob(hash_of(contents)).await, Some((1, false)));
});

database_test!(a_renamed_directory_gets_a_fresh_etag, harness, {
    let owner = harness.account("diretag@example.com", Role::Member).await;
    harness.write(owner.id, "photos/x.txt", b"inside").await;
    let before = harness.resolve(owner.id, "photos").await;

    let after = harness.rename(owner.id, "photos", "pictures").await;

    assert_ne!(
        after.etag, before.etag,
        "a directory's etag is a change token, not a content hash"
    );
});
