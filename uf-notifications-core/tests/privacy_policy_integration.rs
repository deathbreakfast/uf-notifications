//! Valence allow/deny proof for Notification privacy (SEC-NOTIFY-MINT).
//!
//! Schema needles in `product_surface` are smoke. These tests persist through
//! a shared in-memory SQLite router.

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod privacy_helpers;

use chrono::Utc;
use privacy_helpers::{as_system, as_user, setup_shared_db, TEST_USER_A, TEST_USER_B};
use uf_notifications_core::{
    register_notification_audience, send_notification, Notification, SendNotification,
};
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
        subject: None,
    }
}

/// Scoped send of `kind` about `subject` to `recipient`.
fn scoped_send(recipient: &str, kind: &str, subject: &str) -> SendNotification {
    SendNotification {
        user_id: RecordId::new("user", recipient),
        kind: kind.into(),
        title: "Scoped probe".into(),
        message: "body".into(),
        subject: Some(subject.into()),
        ..Default::default()
    }
}

/// Register an audience for `notification` subjects of `kind` that admits only
/// `member`, and only when the check runs as `expected_actor` (no elevation).
fn register_single_member_audience(kind: &str, member: &'static str, expected_actor: &'static str) {
    register_notification_audience("notification", Some(kind), move |_subject, recipient, v| {
        Box::pin(
            async move { Ok(v.actor().user_id() == Some(expected_actor) && recipient == member) },
        )
    });
}

/// Subject record owned by `owner`: only `owner` (and System) can read it.
async fn owned_subject(system: &valence::Valence, owner: &str) -> String {
    format!("notification:{}", mint_row(system, owner).await)
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
        None,
        None,
    )
    .expect("construct notification");
    Notification::upsert(&id, row, system, valence::use_!(r"**Test:** Fixture **Notification** save for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only."))
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

    let loaded = Notification::get(&id, &owner, valence::use_!(r"**Test:** Fixture **Notification** load for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only."))
        .await
        .expect("owner get")
        .expect("owner must see own row");
    assert_eq!(loaded.title(), "Privacy probe");

    loaded
        .get_mutable(&owner, valence::use_!(r"**Test:** Fixture **this data** access in `privacy_policy_integration` so the suite can arrange and assert persistence. CI and developers running the suite only."))
        .set_read_at(Utc::now())
        .expect("owner set_read_at")
        .commit()
        .await
        .expect("owner update");

    let peer_get = Notification::get(&id, &peer, valence::use_!(r"**Test:** Fixture **Notification** load for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only.")).await.ok().flatten();
    assert!(
        peer_get.is_none(),
        "peer must not read another user's notification"
    );

    let owner_again = Notification::get(&id, &owner, valence::use_!(r"**Test:** Fixture **Notification** load for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only."))
        .await
        .expect("owner re-get")
        .expect("row still present");
    match owner_again.get_mutable(&peer, valence::use_!(r"**Test:** Fixture **this data** access in `privacy_policy_integration` so the suite can arrange and assert persistence. CI and developers running the suite only.")).set_read_at(Utc::now()) {
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

    let peer_delete = Notification::delete(&id, &peer, valence::use_!(r"**Test:** Fixture **Notification** remove for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only.")).await;
    assert!(
        peer_delete.is_err(),
        "peer delete must fail under SYSTEM_ONLY, got {peer_delete:?}"
    );

    let owner_delete = Notification::delete(&id, &owner, valence::use_!(r"**Test:** Fixture **Notification** remove for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only.")).await;
    assert!(
        owner_delete.is_err(),
        "owner delete must fail under SYSTEM_ONLY, got {owner_delete:?}"
    );

    Notification::delete(&id, &system, valence::use_!(r"**Test:** Fixture **Notification** remove for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only."))
        .await
        .expect("system may delete");

    let gone = Notification::get(&id, &system, valence::use_!(r"**Test:** Fixture **Notification** load for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only.")).await.ok().flatten();
    assert!(gone.is_none(), "system delete must hide or remove the row");
}

#[tokio::test]
async fn scoped_create_happy_path_records_sender() {
    let base = setup_shared_db().await;
    let system = as_system(&base);
    let sender = as_user(&base, TEST_USER_A);
    let recipient = as_user(&base, TEST_USER_B);
    let subject = owned_subject(&system, TEST_USER_A).await;
    register_single_member_audience("scoped-happy", TEST_USER_B, TEST_USER_A);

    let dto = send_notification(scoped_send(TEST_USER_B, "scoped-happy", &subject), &sender)
        .await
        .expect("sender reads subject and recipient is in its audience");

    let row = Notification::get(&dto.notification_id.to_string(), &recipient, valence::use_!(r"**Test:** Fixture **Notification** load for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only."))
        .await
        .expect("recipient get")
        .expect("recipient sees the scoped notification");
    assert_eq!(row.sender().map(|s| s.id()), Some(TEST_USER_A));
    assert_eq!(row.subject().map(String::as_str), Some(subject.as_str()));
}

#[tokio::test]
async fn scoped_create_forged_sender_denied_sad() {
    let base = setup_shared_db().await;
    let system = as_system(&base);
    let actor = as_user(&base, TEST_USER_A);
    let subject = owned_subject(&system, TEST_USER_A).await;
    register_single_member_audience("scoped-forged", TEST_USER_B, TEST_USER_A);

    let forged = Notification::new(
        RecordId::new("user", TEST_USER_B),
        "scoped-forged".into(),
        "Forged".into(),
        "body".into(),
        None,
        None,
        None,
        Utc::now(),
        Some(RecordId::new("user", TEST_USER_B)),
        Some(subject),
    )
    .expect("construct notification");
    let result = Notification::upsert(&Uuid::new_v4().to_string(), forged, &actor, valence::use_!(r"**Test:** Fixture **Notification** save for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only.")).await;
    assert!(
        result.is_err(),
        "sender other than the actor must be denied, got {result:?}"
    );
}

#[tokio::test]
async fn scoped_create_unreadable_subject_denied_sad() {
    let base = setup_shared_db().await;
    let system = as_system(&base);
    let outsider = as_user(&base, TEST_USER_B);
    // Owned by A, so B cannot read it; the audience would admit B.
    let subject = owned_subject(&system, TEST_USER_A).await;
    register_single_member_audience("scoped-unreadable", TEST_USER_B, TEST_USER_B);

    let result = send_notification(
        scoped_send(TEST_USER_B, "scoped-unreadable", &subject),
        &outsider,
    )
    .await;
    assert!(
        result.is_err(),
        "subject the actor cannot read must be denied, got {result:?}"
    );
}

#[tokio::test]
async fn scoped_create_recipient_outside_audience_denied_sad() {
    let base = setup_shared_db().await;
    let system = as_system(&base);
    let sender = as_user(&base, TEST_USER_A);
    let subject = owned_subject(&system, TEST_USER_A).await;
    // Audience admits only A, so B is outside it.
    register_single_member_audience("scoped-outside", TEST_USER_A, TEST_USER_A);

    let result = send_notification(
        scoped_send(TEST_USER_B, "scoped-outside", &subject),
        &sender,
    )
    .await;
    assert!(
        result.is_err(),
        "recipient outside the audience must be denied, got {result:?}"
    );
}

#[tokio::test]
async fn scoped_create_without_audience_check_only_self_sad_and_happy() {
    let base = setup_shared_db().await;
    let system = as_system(&base);
    let sender = as_user(&base, TEST_USER_A);
    let subject = owned_subject(&system, TEST_USER_A).await;

    let to_peer = send_notification(
        scoped_send(TEST_USER_B, "scoped-unregistered", &subject),
        &sender,
    )
    .await;
    assert!(
        to_peer.is_err(),
        "no registered audience must refuse other recipients, got {to_peer:?}"
    );

    send_notification(
        scoped_send(TEST_USER_A, "scoped-unregistered", &subject),
        &sender,
    )
    .await
    .expect("no registered audience still allows recipient == actor");
}

#[tokio::test]
async fn system_scoped_and_legacy_unscoped_create_happy_path() {
    let base = setup_shared_db().await;
    let system = as_system(&base);
    let peer = as_user(&base, TEST_USER_B);
    let subject = owned_subject(&system, TEST_USER_A).await;

    let dto = send_notification(scoped_send(TEST_USER_B, "scoped-system", &subject), &system)
        .await
        .expect("System may create scoped notifications");
    let row = Notification::get(&dto.notification_id.to_string(), &system, valence::use_!(r"**Test:** Fixture **Notification** load for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only."))
        .await
        .expect("system get")
        .expect("row present");
    assert!(row.sender().is_none(), "System sends record no sender");

    send_notification(sample_send(TEST_USER_A), &peer)
        .await
        .expect("legacy row without subject stays allowed in phase A");
}

#[tokio::test]
async fn legacy_create_forged_sender_denied_sad() {
    let base = setup_shared_db().await;
    let actor = as_user(&base, TEST_USER_A);

    let forged = Notification::new(
        RecordId::new("user", TEST_USER_B),
        "legacy-forged".into(),
        "Forged".into(),
        "body".into(),
        None,
        None,
        None,
        Utc::now(),
        Some(RecordId::new("user", TEST_USER_B)),
        None,
    )
    .expect("construct notification");
    let result = Notification::upsert(&Uuid::new_v4().to_string(), forged, &actor, valence::use_!(r"**Test:** Fixture **Notification** save for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only.")).await;
    assert!(
        result.is_err(),
        "legacy row naming another user as sender must be denied, got {result:?}"
    );

    let dto = send_notification(sample_send(TEST_USER_B), &actor)
        .await
        .expect("legacy send from the actor still records the actor as sender");
    let row = Notification::get(&dto.notification_id.to_string(), &as_system(&base), valence::use_!(r"**Test:** Fixture **Notification** load for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only."))
        .await
        .expect("system get")
        .expect("row present");
    assert_eq!(row.sender().map(|s| s.id()), Some(TEST_USER_A));
}
