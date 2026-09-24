mod common;

use chrono::TimeDelta;
use reqwest::StatusCode;
use roxycloud_client::Remote;
use roxycloud_client::remote::RemoteError;
use roxycloud_core::role::Role;
use roxycloud_core::user::User;

use common::{Harness, serve};

async fn connect(harness: &Harness, account: &User) -> Remote {
    let base = serve(harness.state.clone()).await;
    let token = harness.state.sessions.issue(account.id).expect("a token");
    Remote::new(&base, token).expect("a client")
}

database_test!(the_client_reads_the_trash_with_its_expiry, harness, {
    let owner = harness.account("expiry@example.com", Role::Member).await;
    let file = harness.write(owner.id, "a.txt", b"on a clock").await;
    harness.trash(&file).await;
    let mut kept_a_month = harness.state.clone();
    kept_a_month.trash_retention = Some(TimeDelta::days(30));
    let base = serve(kept_a_month).await;
    let token = harness.state.sessions.issue(owner.id).expect("a token");
    let remote = Remote::new(&base, token).expect("a client");

    let trashed = remote.trash().await.expect("listing the trash");

    assert_eq!(trashed.len(), 1);
    assert_eq!(trashed[0].node.name, "a.txt");
    let deleted = trashed[0]
        .node
        .deleted_at
        .expect("a trashed node says when");
    assert_eq!(
        trashed[0].expires_at,
        Some(deleted + TimeDelta::days(30)),
        "the expiry survives the trip, so the desktop panel can show it"
    );
});

database_test!(the_client_restores_and_purges_from_the_trash, harness, {
    let owner = harness.account("restore@example.com", Role::Member).await;
    let back = harness.write(owner.id, "back.txt", b"comes back").await;
    let gone = harness.write(owner.id, "gone.txt", b"goes for good").await;
    harness.trash(&back).await;
    harness.trash(&gone).await;
    let remote = connect(&harness, &owner).await;

    let restored = remote.restore(back.id).await.expect("restoring");
    remote.purge(gone.id).await.expect("purging");

    assert_eq!(restored.name, "back.txt");
    assert!(harness.trashed(owner.id).await.is_empty());
    assert_eq!(harness.resolve(owner.id, "back.txt").await.id, back.id);
});

database_test!(
    a_restore_onto_a_taken_name_carries_the_server_explanation,
    harness,
    {
        let owner = harness.account("taken@example.com", Role::Member).await;
        let first = harness.write(owner.id, "a.txt", b"the first one").await;
        harness.trash(&first).await;
        harness.write(owner.id, "a.txt", b"its replacement").await;
        let remote = connect(&harness, &owner).await;

        let refused = remote.restore(first.id).await;

        match refused {
            Err(RemoteError::Refused { status, message }) => {
                assert_eq!(status, StatusCode::CONFLICT);
                assert_eq!(message, "a.txt already exists");
            }
            other => panic!("expected the server's own words, got {other:?}"),
        }
    }
);

database_test!(a_reader_emptying_the_trash_is_told_why, harness, {
    let reader = harness.account("reader@example.com", Role::Reader).await;
    let remote = connect(&harness, &reader).await;

    let refused = remote.empty_trash().await;

    match refused {
        Err(error @ RemoteError::Refused { .. }) => {
            assert_eq!(error.status(), Some(StatusCode::FORBIDDEN));
            assert_eq!(error.to_string(), "this account may not write");
        }
        other => panic!("expected the server's own words, got {other:?}"),
    }
});

database_test!(the_client_empties_the_trash, harness, {
    let owner = harness.account("empty@example.com", Role::Member).await;
    for name in ["a.txt", "b.txt"] {
        let node = harness.write(owner.id, name, name.as_bytes()).await;
        harness.trash(&node).await;
    }
    let remote = connect(&harness, &owner).await;

    remote.empty_trash().await.expect("emptying");

    assert!(harness.trashed(owner.id).await.is_empty());
});

database_test!(the_client_lists_downloads_and_restores_versions, harness, {
    let owner = harness.account("versions@example.com", Role::Member).await;
    harness
        .write(owner.id, "notes/plan.md", b"the first plan")
        .await;
    harness
        .write(owner.id, "notes/plan.md", b"the plan that replaced it")
        .await;
    let remote = connect(&harness, &owner).await;

    let versions = remote
        .list_versions("notes/plan.md")
        .await
        .expect("listing the versions");
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].size, 14);

    let destination = tempfile::NamedTempFile::new().expect("a scratch file");
    remote
        .download_version("notes/plan.md", versions[0].id, destination.path())
        .await
        .expect("downloading the version");
    assert_eq!(
        std::fs::read(destination.path()).expect("reading it back"),
        b"the first plan"
    );

    let restored = remote
        .restore_version("notes/plan.md", versions[0].id)
        .await
        .expect("restoring the version");
    assert_eq!(restored.size, 14);
    assert_eq!(
        remote
            .read("notes/plan.md")
            .await
            .expect("reading the file"),
        &b"the first plan"[..]
    );
});

database_test!(a_version_of_another_file_is_not_found, harness, {
    let owner = harness.account("elsewhere@example.com", Role::Member).await;
    harness.write(owner.id, "a.txt", b"one").await;
    harness.write(owner.id, "a.txt", b"two").await;
    harness.write(owner.id, "b.txt", b"unrelated").await;
    let remote = connect(&harness, &owner).await;
    let versions = remote.list_versions("a.txt").await.expect("listing");

    let wrong = remote.restore_version("b.txt", versions[0].id).await;

    assert!(matches!(wrong, Err(RemoteError::NotFound(_))));
});
