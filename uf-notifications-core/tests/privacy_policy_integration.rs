//! Valence allow/deny proof for Notification privacy (SEC-NOTIFY-MINT).
//!
//! Schema needles in `product_surface` are smoke. These tests persist through
//! a shared in-memory SQLite router.

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod privacy_helpers;

use chrono::Utc;
use privacy_helpers::{as_system, as_user, setup_shared_db, TEST_USER_A, TEST_USER_B};
use uf_notifications_core::{send_notification, Notification, SendNotification};
use uuid::Uuid;
use valence::{Model, RecordId};

fn sample_send(user_id: &str) -> SendNotification {
    SendNotification {
        user_id: RecordId::new("user", user_id),
        kind: "test".into(),
        title: "Privacy probe".into(),
        message: "body".into(),
        url: Some("/notifications".into()),
        data_json: None,
    }
}

async fn mint_row(system: &valence::Valence, owner: &str) -> String {
    let id = Uuid::new_v4().to_string();
    let row = Notification::new(
        RecordId::new("user", owner),
        "test".into(),
        "Privacy probe".into(),
        "body".into(),
        Some("/notifications".into()),
        None,
        None,
        Utc::now(),
    )
    .expect("construct notification");
    Notification::upsert_used(&id, row, system, valence::use_!(r"**Test:** Fixture **Notification** save for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only."))
        .await
        .expect("system may create");
    id
}

#[tokio::test]
async fn system_and_session_mint_happy_path() {
    let base = setup_shared_db().await;
    let system = as_system(&base);
    let owner = as_user(&base, TEST_USER_A);

    send_notification(sample_send(TEST_USER_A), &system)
        .await
        .expect("system send_notification must persist");

    send_notification(sample_send(TEST_USER_B), &owner)
        .await
        .expect("session Valence may mint under AUTHENTICATED create (product fanout)");
}

#[tokio::test]
async fn owner_read_update_happy_peer_denied_sad() {
    let base = setup_shared_db().await;
    let system = as_system(&base);
    let owner = as_user(&base, TEST_USER_A);
    let peer = as_user(&base, TEST_USER_B);

    let id = mint_row(&system, TEST_USER_A).await;

    let loaded = Notification::get_used(&id, &owner, valence::use_!(r"**Test:** Fixture **Notification** load for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only."))
        .await
        .expect("owner get")
        .expect("owner must see own row");
    assert_eq!(loaded.title(), "Privacy probe");

    loaded
        .get_mutable_used(&owner, valence::use_!(r"**Test:** Fixture **this data** access in `privacy_policy_integration` so the suite can arrange and assert persistence. CI and developers running the suite only."))
        .set_read_at(Utc::now())
        .expect("owner set_read_at")
        .commit()
        .await
        .expect("owner update");

    let peer_get = Notification::get_used(&id, &peer, valence::use_!(r"**Test:** Fixture **Notification** load for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only.")).await.ok().flatten();
    assert!(
        peer_get.is_none(),
        "peer must not read another user's notification"
    );

    let owner_again = Notification::get_used(&id, &owner, valence::use_!(r"**Test:** Fixture **Notification** load for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only."))
        .await
        .expect("owner re-get")
        .expect("row still present");
    match owner_again.get_mutable_used(&peer, valence::use_!(r"**Test:** Fixture **this data** access in `privacy_policy_integration` so the suite can arrange and assert persistence. CI and developers running the suite only.")).set_read_at(Utc::now()) {
        Err(_) => {}
        Ok(mutable) => {
            assert!(
                mutable.commit().await.is_err(),
                "peer must not commit an update on another user's notification"
            );
        }
    }
}

#[tokio::test]
async fn peer_and_session_delete_deny_system_delete_happy() {
    let base = setup_shared_db().await;
    let system = as_system(&base);
    let owner = as_user(&base, TEST_USER_A);
    let peer = as_user(&base, TEST_USER_B);

    let id = mint_row(&system, TEST_USER_A).await;

    let peer_delete = Notification::delete_used(&id, &peer, valence::use_!(r"**Test:** Fixture **Notification** remove for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only.")).await;
    assert!(
        peer_delete.is_err(),
        "peer delete must fail under SYSTEM_ONLY, got {peer_delete:?}"
    );

    let owner_delete = Notification::delete_used(&id, &owner, valence::use_!(r"**Test:** Fixture **Notification** remove for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only.")).await;
    assert!(
        owner_delete.is_err(),
        "owner delete must fail under SYSTEM_ONLY, got {owner_delete:?}"
    );

    Notification::delete_used(&id, &system, valence::use_!(r"**Test:** Fixture **Notification** remove for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only."))
        .await
        .expect("system may delete");

    let gone = Notification::get_used(&id, &system, valence::use_!(r"**Test:** Fixture **Notification** load for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only.")).await.ok().flatten();
    assert!(gone.is_none(), "system delete must hide or remove the row");
}
