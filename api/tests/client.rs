mod common;

use chrono::TimeDelta;
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
            Err(RemoteError::Refused(message)) => assert_eq!(message, "a.txt already exists"),
            other => panic!("expected the server's own words, got {other:?}"),
        }
    }
);

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
