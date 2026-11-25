//! Notification kinds, preference storage, privacy and account-deletion cascade.
//!
//! Persists through the same in-memory SQLite router as
//! `privacy_policy_integration`.

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod privacy_helpers;

use std::sync::Arc;

use privacy_helpers::{as_system, as_user, setup_shared_db, TEST_USER_A, TEST_USER_B};
use uf_notifications_core::{
    effective_channels, load_preferences, load_settings, register_contact_verification,
    save_settings, send_notification, set_channel, Channel, Channels, ContactVerification, Digest,
    NotificationKind, NotificationPreference, NotificationSettings, PreferenceError,
    SendNotification, Settings,
};
use valence::{Model, RecordId, RecordPredicate, Valence};

uf_notifications_core::inventory::submit! {
    NotificationKind {
        id: "preftest.assigned",
        app: "preftest",
        label: "Assigned to you",
        description: "Someone assigned you something.",
        defaults: Channels { in_app: true, email: true, text: false },
        asks_action: true,
        urgent: false,
    }
}

uf_notifications_core::inventory::submit! {
    NotificationKind {
        id: "preftest.digest",
        app: "preftest",
        label: "Weekly summary",
        description: "What happened this week.",
        defaults: Channels::IN_APP_ONLY,
        asks_action: false,
        urgent: false,
    }
}

fn user(id: &str) -> RecordId {
    RecordId::new("user", id)
}

/// Only `TEST_USER_B` has a verified email and phone, so tests about user A
/// see the "not verified" path whatever order the tests run in.
struct OnlyUserBVerified;

#[async_trait::async_trait]
impl ContactVerification for OnlyUserBVerified {
    async fn email_verified(&self, user: &RecordId, _: &Valence) -> valence::Result<bool> {
        Ok(user.id() == TEST_USER_B)
    }
    async fn phone_verified(&self, user: &RecordId, _: &Valence) -> valence::Result<bool> {
        Ok(user.id() == TEST_USER_B)
    }
}

fn send(kind: &str) -> SendNotification {
    SendNotification {
        user_id: user(TEST_USER_A),
        kind: kind.into(),
        title: "Kind probe".into(),
        message: "body".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn registered_kind_fills_app_and_label_happy_path() {
    let base = setup_shared_db().await;
    let a = as_user(&base, TEST_USER_A);

    let dto = send_notification(send("preftest.assigned"), &a)
        .await
        .expect("send to self");
    assert_eq!(dto.kind_id.as_deref(), Some("preftest.assigned"));
    assert_eq!(dto.kind_label.as_deref(), Some("Assigned to you"));
    assert_eq!(dto.app_id.as_deref(), Some("preftest"));
    assert!(dto.asks_action);
}

#[tokio::test]
async fn unregistered_kind_is_stored_in_app_only_sad_path() {
    let base = setup_shared_db().await;
    let a = as_user(&base, TEST_USER_A);

    let dto = send_notification(send("preftest.never-registered"), &a)
        .await
        .expect("unregistered kinds are still stored");
    assert_eq!(dto.kind_id.as_deref(), Some("preftest.never-registered"));
    assert_eq!(dto.kind_label, None);
    assert_eq!(dto.app_id, None);
    assert!(!dto.asks_action);

    let stored = uf_notifications_core::Notification::get(&dto.notification_id.to_string(), &a, valence::use_!(r"**Test:** Fixture **Notification** read for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only."))
        .await
        .expect("read own row")
        .expect("row persisted");
    assert_eq!(stored.kind(), "preftest.never-registered");

    let channels = effective_channels(&a, &user(TEST_USER_A), "preftest.never-registered")
        .await
        .expect("effective channels");
    assert_eq!(channels, Channels::IN_APP_ONLY);
}

#[tokio::test]
async fn stored_preference_overrides_defaults_happy_path() {
    let base = setup_shared_db().await;
    let a = as_user(&base, TEST_USER_A);
    let me = user(TEST_USER_A);

    // Defaults: in-app on, email on but masked until verified.
    let before = effective_channels(&a, &me, "preftest.assigned")
        .await
        .unwrap();
    assert_eq!(before, Channels::IN_APP_ONLY);

    let saved = set_channel(&a, &me, "preftest.assigned", Channel::InApp, false)
        .await
        .expect("owner may save");
    assert_eq!(
        saved,
        Channels {
            in_app: false,
            email: true,
            text: false
        },
        "first change starts from the kind's defaults"
    );

    let after = effective_channels(&a, &me, "preftest.assigned")
        .await
        .unwrap();
    assert!(!after.in_app, "stored row wins over defaults");

    let rows = load_preferences(&a, &me).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "preftest.assigned");

    // A second change updates the same row.
    set_channel(&a, &me, "preftest.assigned", Channel::InApp, true)
        .await
        .unwrap();
    assert_eq!(load_preferences(&a, &me).await.unwrap().len(), 1);

    // Untouched kinds keep their defaults.
    let other = effective_channels(&a, &me, "preftest.digest")
        .await
        .unwrap();
    assert_eq!(other, Channels::IN_APP_ONLY);
}

#[tokio::test]
async fn verified_contacts_unmask_email_and_text_happy_path() {
    register_contact_verification(Arc::new(OnlyUserBVerified));
    let base = setup_shared_db().await;
    let b = as_user(&base, TEST_USER_B);
    let me = user(TEST_USER_B);

    let defaults = effective_channels(&b, &me, "preftest.assigned")
        .await
        .unwrap();
    assert!(defaults.email, "verified email keeps the email default");
    assert!(!defaults.text, "text default is off");

    set_channel(&b, &me, "preftest.assigned", Channel::Text, true)
        .await
        .unwrap();
    let after = effective_channels(&b, &me, "preftest.assigned")
        .await
        .unwrap();
    assert!(after.text, "verified phone lets text through");
}

#[tokio::test]
async fn unknown_kind_cannot_be_saved_sad_path() {
    let base = setup_shared_db().await;
    let a = as_user(&base, TEST_USER_A);
    let err = set_channel(
        &a,
        &user(TEST_USER_A),
        "preftest.nope",
        Channel::Email,
        true,
    )
    .await
    .expect_err("unregistered kind");
    assert!(matches!(err, PreferenceError::UnknownKind), "{err:?}");
}

#[tokio::test]
async fn settings_default_then_round_trip_and_validate() {
    let base = setup_shared_db().await;
    let a = as_user(&base, TEST_USER_A);
    let me = user(TEST_USER_A);

    let defaults = load_settings(&a, &me).await.unwrap();
    assert_eq!(defaults, Settings::default());
    assert_eq!(defaults.digest, Digest::Instant);
    assert!(!defaults.quiet_enabled);

    let wanted = Settings {
        digest: Digest::Weekly,
        quiet_enabled: true,
        quiet_start: "21:30".into(),
        quiet_end: "06:45".into(),
        time_zone: "Europe/Dublin".into(),
    };
    save_settings(&a, &me, wanted.clone()).await.unwrap();
    assert_eq!(load_settings(&a, &me).await.unwrap(), wanted);

    let bad_time = save_settings(
        &a,
        &me,
        Settings {
            quiet_start: "25:00".into(),
            ..wanted.clone()
        },
    )
    .await;
    assert!(matches!(bad_time, Err(PreferenceError::InvalidSettings(_))));
    let bad_tz = save_settings(
        &a,
        &me,
        Settings {
            time_zone: "  ".into(),
            ..wanted
        },
    )
    .await;
    assert!(matches!(bad_tz, Err(PreferenceError::InvalidSettings(_))));
}

#[tokio::test]
async fn peer_cannot_read_or_write_preferences_sad_path() {
    let base = setup_shared_db().await;
    let a = as_user(&base, TEST_USER_A);
    let b = as_user(&base, TEST_USER_B);
    let alice = user(TEST_USER_A);

    set_channel(&a, &alice, "preftest.assigned", Channel::Email, false)
        .await
        .unwrap();
    save_settings(
        &a,
        &alice,
        Settings {
            digest: Digest::Daily,
            ..Settings::default()
        },
    )
    .await
    .unwrap();

    // B's listing of A's rows comes back empty.
    let peer_rows = load_preferences(&b, &alice).await.unwrap_or_default();
    assert!(peer_rows.is_empty(), "peer saw {peer_rows:?}");
    // B's read of A's settings is denied or hidden, never A's values.
    if let Ok(s) = load_settings(&b, &alice).await {
        assert_eq!(s, Settings::default(), "peer must not see A's settings");
    }

    // B can't change A's choices or settings.
    assert!(
        set_channel(&b, &alice, "preftest.assigned", Channel::Email, true)
            .await
            .is_err(),
        "peer write to A's preference must fail"
    );
    assert!(
        save_settings(&b, &alice, Settings::default())
            .await
            .is_err(),
        "peer write to A's settings must fail"
    );

    // A's rows are unchanged.
    let rows = load_preferences(&a, &alice).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].1.email);
    assert_eq!(
        load_settings(&a, &alice).await.unwrap().digest,
        Digest::Daily
    );
}

async fn rows_for(system: &Valence, owner: &RecordId) -> (usize, usize) {
    let prefs: Vec<NotificationPreference> = NotificationPreference::query(system, valence::use_!(r"**Test:** Fixture **Notification Preference** list for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only."))
        .where_user(RecordPredicate::Equals(owner.clone()))
        .await
        .unwrap();
    let settings: Vec<NotificationSettings> = NotificationSettings::query(system, valence::use_!(r"**Test:** Fixture **Notification Settings** list for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only."))
        .where_user(RecordPredicate::Equals(owner.clone()))
        .await
        .unwrap();
    (prefs.len(), settings.len())
}

#[tokio::test]
async fn deleting_user_deletes_preferences_and_settings_happy_path() {
    let base = setup_shared_db().await;
    let a = as_user(&base, TEST_USER_A);
    let b = as_user(&base, TEST_USER_B);
    let system = as_system(&base);

    for (v, id) in [(&a, TEST_USER_A), (&b, TEST_USER_B)] {
        set_channel(v, &user(id), "preftest.assigned", Channel::Email, false)
            .await
            .unwrap();
        save_settings(v, &user(id), Settings::default())
            .await
            .unwrap();
    }
    assert_eq!(rows_for(&system, &user(TEST_USER_A)).await, (1, 1));

    // Account deletion runs as a System job; the cascade follows `on_delete`.
    valence::deletion::delete_entity_now("user", TEST_USER_A, &system)
        .await
        .expect("delete user A");

    assert_eq!(
        rows_for(&system, &user(TEST_USER_A)).await,
        (0, 0),
        "both tables' rows go with the account"
    );
    assert_eq!(
        rows_for(&system, &user(TEST_USER_B)).await,
        (1, 1),
        "other people's rows stay"
    );
}
