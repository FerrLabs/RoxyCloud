mod common;

use roxycloud_api::app_passwords::{authenticate, list, mint, revoke};
use roxycloud_core::role::Role;
use roxycloud_core::user::Email;

async fn minted(harness: &common::Harness, user: uuid::Uuid, name: &str) -> (uuid::Uuid, String) {
    let mut tx = harness.state.db.begin().await.expect("begin");
    let minted = mint(&mut tx, user, name).await.expect("minting");
    tx.commit().await.expect("commit");
    (minted.password.id, minted.secret)
}

fn email(raw: &str) -> Email {
    raw.parse().expect("a valid email")
}

database_test!(a_minted_secret_authenticates_its_owner, harness, {
    let owner = harness.account("dav@example.com", Role::Member).await;
    let (_, secret) = minted(&harness, owner.id, "rclone on the laptop").await;

    let authenticated = authenticate(&harness.state.db, &email("dav@example.com"), &secret).await;

    assert_eq!(
        authenticated.map(|user| user.id),
        Some(owner.id),
        "the credential a client stores in plain text is not the account password, but it is the account"
    );
});

database_test!(the_secret_is_never_stored_and_never_repeated, harness, {
    let owner = harness.account("once@example.com", Role::Member).await;
    let (id, secret) = minted(&harness, owner.id, "shown once").await;

    let stored = sqlx::query_scalar::<_, String>("SELECT hash FROM app_passwords WHERE id = $1")
        .bind(id)
        .fetch_one(&harness.state.db)
        .await
        .expect("reading the row");

    assert_ne!(stored, secret, "the secret itself must not reach the table");
    assert!(!stored.contains(&secret));
    let listed = list(&harness.state.db, owner.id).await.expect("listing");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "shown once");
});

database_test!(two_secrets_never_come_out_the_same, harness, {
    let owner = harness.account("entropy@example.com", Role::Member).await;

    let (_, first) = minted(&harness, owner.id, "one").await;
    let (_, second) = minted(&harness, owner.id, "two").await;

    assert_ne!(first, second);
    assert_eq!(first.len(), 64, "32 bytes of randomness, hex encoded");
});

database_test!(a_revoked_secret_stops_working_at_once, harness, {
    let owner = harness.account("revoked@example.com", Role::Member).await;
    let (id, secret) = minted(&harness, owner.id, "a laptop that was lost").await;

    revoke(&harness.state.db, owner.id, id)
        .await
        .expect("revoking");

    assert!(
        authenticate(&harness.state.db, &email("revoked@example.com"), &secret)
            .await
            .is_none(),
        "revoking is the whole point, and it cannot wait for a cache to expire"
    );
    assert!(
        list(&harness.state.db, owner.id)
            .await
            .expect("listing")
            .is_empty()
    );
});

database_test!(
    one_account_cannot_revoke_another_accounts_credential,
    harness,
    {
        let owner = harness.account("mine@example.com", Role::Member).await;
        let stranger = harness.account("theirs@example.com", Role::Member).await;
        let (id, secret) = minted(&harness, owner.id, "mine").await;

        assert!(revoke(&harness.state.db, stranger.id, id).await.is_err());
        assert!(
            authenticate(&harness.state.db, &email("mine@example.com"), &secret)
                .await
                .is_some()
        );
    }
);

database_test!(a_wrong_secret_and_a_wrong_account_both_fail, harness, {
    let owner = harness.account("wrong@example.com", Role::Member).await;
    let stranger = harness.account("other@example.com", Role::Member).await;
    let (_, secret) = minted(&harness, owner.id, "mine").await;

    assert!(
        authenticate(
            &harness.state.db,
            &email("wrong@example.com"),
            "not the secret"
        )
        .await
        .is_none()
    );
    assert!(
        authenticate(&harness.state.db, &email("other@example.com"), &secret)
            .await
            .is_none(),
        "a secret is bound to the account that minted it"
    );
    let _ = stranger;
});

database_test!(a_disabled_account_cannot_authenticate_with_one, harness, {
    let owner = harness.account("disabled@example.com", Role::Member).await;
    let (_, secret) = minted(&harness, owner.id, "still on the laptop").await;
    sqlx::query("UPDATE users SET disabled_at = now() WHERE id = $1")
        .bind(owner.id)
        .execute(&harness.state.db)
        .await
        .expect("disabling the account");

    assert!(
        authenticate(&harness.state.db, &email("disabled@example.com"), &secret)
            .await
            .is_none(),
        "the credential outlives nothing the account itself does not"
    );
});

database_test!(using_a_secret_records_when, harness, {
    let owner = harness.account("used@example.com", Role::Member).await;
    let (_, secret) = minted(&harness, owner.id, "in use").await;
    assert!(
        list(&harness.state.db, owner.id).await.expect("listing")[0]
            .last_used_at
            .is_none()
    );

    authenticate(&harness.state.db, &email("used@example.com"), &secret)
        .await
        .expect("authenticating");

    assert!(
        list(&harness.state.db, owner.id).await.expect("listing")[0]
            .last_used_at
            .is_some(),
        "an unused credential should be visible as one, so it can be cleaned up"
    );
});
