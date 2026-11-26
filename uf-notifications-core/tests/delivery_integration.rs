//! Email and text delivery, digest, retention and phone verification against
//! a recording fake channel.
//!
//! Persists through the same in-memory SQLite router as
//! `privacy_policy_integration`. Delivery runs as the `svc:notifications`
//! actor, the digest and retention jobs as System, phone verification as the
//! person.

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod privacy_helpers;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, TimeZone, Timelike, Utc};
use privacy_helpers::{
    as_system, as_user, seed_user, setup_valence_with, TEST_USER_A, TEST_USER_B,
};
use uf_notifications_core::channels::{
    ChannelError, ChannelKind, Contact, NotificationChannel, NotificationChannels,
    OutgoingNotification, PHONE_CODE_KIND_ID,
};
use uf_notifications_core::delivery::{deliver_with_valence, ChannelOutcome, DeliveryReport};
use uf_notifications_core::digest::digest_with_valence;
use uf_notifications_core::phone::{
    confirm_phone_verification_at, start_phone_verification_at, PhoneVerificationError,
};
use uf_notifications_core::retention::retention_with_valence;
use uf_notifications_core::{
    channel_configured, contact_verified, delivery_actor, register_contact_verification,
    save_settings, send_notification, set_channel, Channel, Channels, ContactVerification, Digest,
    Notification, NotificationContact, NotificationKind, SendNotification, Settings,
};
use valence::{Actor, Model, RecordId, Valence};

uf_notifications_core::inventory::submit! {
    NotificationKind {
        id: "deltest.assigned",
        app: "deltest",
        label: "Assigned to you",
        description: "Someone assigned you something.",
        defaults: Channels { in_app: true, email: true, text: true },
        asks_action: true,
        urgent: false,
    }
}

uf_notifications_core::inventory::submit! {
    NotificationKind {
        id: "deltest.approval",
        app: "deltest",
        label: "Approval needed",
        description: "Someone needs your approval.",
        defaults: Channels { in_app: true, email: true, text: false },
        asks_action: true,
        urgent: true,
    }
}

/// Every email verified as `<id>@example.test`; phones come from this crate.
struct EmailVerified;

#[async_trait::async_trait]
impl ContactVerification for EmailVerified {
    async fn email_verified(&self, _: &RecordId, _: &Valence) -> valence::Result<bool> {
        Ok(true)
    }
    async fn verified_email(
        &self,
        user: &RecordId,
        _: &Valence,
    ) -> valence::Result<Option<String>> {
        Ok(Some(format!("{}@example.test", user.id())))
    }
}

/// Records every message; fails while `fail` is set.
struct Recorder {
    kind: ChannelKind,
    sent: Mutex<Vec<(Contact, OutgoingNotification)>>,
    fail: AtomicBool,
}

impl Recorder {
    fn new(kind: ChannelKind) -> Arc<Self> {
        Arc::new(Self {
            kind,
            sent: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
        })
    }

    fn sent(&self) -> Vec<(Contact, OutgoingNotification)> {
        self.sent.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl NotificationChannel for Recorder {
    fn kind(&self) -> ChannelKind {
        self.kind
    }
    async fn deliver(&self, to: &Contact, msg: &OutgoingNotification) -> Result<(), ChannelError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(ChannelError("provider unavailable".into()));
        }
        self.sent.lock().unwrap().push((to.clone(), msg.clone()));
        Ok(())
    }
}

struct Lab {
    system: Valence,
    email: Arc<Recorder>,
    text: Arc<Recorder>,
}

impl Lab {
    fn delivery(&self) -> Valence {
        self.system.with_actor(delivery_actor())
    }

    fn user(&self, id: &str) -> Valence {
        as_user(&self.system, id)
    }
}

async fn lab() -> Lab {
    register_contact_verification(Arc::new(EmailVerified));
    let email = Recorder::new(ChannelKind::Email);
    let text = Recorder::new(ChannelKind::Text);
    let channels = NotificationChannels::new()
        .with(email.clone())
        .with(text.clone());
    let system = setup_valence_with(|b| NotificationChannels::install(b, channels)).await;
    seed_user(TEST_USER_A, &system).await;
    seed_user(TEST_USER_B, &system).await;
    Lab {
        system: as_system(&system),
        email,
        text,
    }
}

fn user(id: &str) -> RecordId {
    RecordId::new("user", id)
}

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap()
}

async fn send(v: &Valence, to: &str, kind: &str, title: &str) -> String {
    send_notification(
        SendNotification {
            user_id: user(to),
            kind: kind.into(),
            title: title.into(),
            message: format!("{title} body"),
            url: Some("/inbox".into()),
            ..Default::default()
        },
        v,
    )
    .await
    .expect("send")
    .notification_id
    .to_string()
}

async fn row(v: &Valence, id: &str) -> Option<Notification> {
    Notification::get(id, v, valence::use_!(r"**Test:** Read a **Notification** row back in `tests` to check delivery fields. CI and developers running the suite only.")).await.unwrap()
}

async fn set_settings(v: &Valence, who: &str, settings: Settings) {
    save_settings(v, &user(who), settings)
        .await
        .expect("save settings");
}

fn quiet_nights() -> Settings {
    Settings {
        quiet_enabled: true,
        quiet_start: "22:00".into(),
        quiet_end: "07:00".into(),
        time_zone: "UTC".into(),
        ..Settings::default()
    }
}

/// Text to user A goes to a verified phone.
async fn verify_phone(lab: &Lab, who: &str, number: &str, now: DateTime<Utc>) {
    let v = lab.user(who);
    start_phone_verification_at(&v, &user(who), number, now)
        .await
        .expect("start");
    let code = last_code(&lab.text);
    confirm_phone_verification_at(&v, &user(who), &code, now)
        .await
        .expect("confirm");
}

fn last_code(text: &Recorder) -> String {
    let (_, msg) = text.sent().last().cloned().expect("a code was sent");
    assert_eq!(msg.kind_id, PHONE_CODE_KIND_ID);
    msg.message
        .split(|c: char| !c.is_ascii_digit())
        .find(|w| w.len() == 6)
        .expect("6-digit code")
        .to_string()
}

#[tokio::test]
async fn email_on_delivers_once_and_records_sent_at() {
    let lab = lab().await;
    let id = send(&lab.system, TEST_USER_A, "deltest.assigned", "Assigned").await;
    let now = utc(2026, 3, 2, 12, 0);

    let report = deliver_with_valence(&lab.delivery(), &id, now)
        .await
        .unwrap();
    assert_eq!(report.email, ChannelOutcome::Sent);
    // Text is on for the kind but there is no verified phone yet.
    assert_eq!(report.text, ChannelOutcome::Off);
    let sent = lab.email.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].0,
        Contact::Email(format!("{TEST_USER_A}@example.test"))
    );
    assert_eq!(sent[0].1.title, "Assigned");
    assert_eq!(sent[0].1.url.as_deref(), Some("/inbox"));
    let stored = row(&lab.system, &id).await.unwrap();
    assert_eq!(stored.email_sent_at().copied(), Some(now));
    assert!(stored.delivery_error().is_none());

    // A retry never sends twice.
    let again = deliver_with_valence(&lab.delivery(), &id, now)
        .await
        .unwrap();
    assert_eq!(again.email, ChannelOutcome::AlreadySent);
    assert_eq!(lab.email.sent().len(), 1);
}

#[tokio::test]
async fn quiet_hours_hold_until_the_window_ends() {
    let lab = lab().await;
    set_settings(&lab.user(TEST_USER_A), TEST_USER_A, quiet_nights()).await;
    let id = send(&lab.system, TEST_USER_A, "deltest.approval", "Approve").await;

    let report = deliver_with_valence(&lab.delivery(), &id, utc(2026, 3, 2, 23, 30))
        .await
        .unwrap();
    // Urgent kinds skip the digest but still wait for quiet hours to end.
    assert_eq!(report.email, ChannelOutcome::Held(utc(2026, 3, 3, 7, 0)));
    assert_eq!(report.held_until(), Some(utc(2026, 3, 3, 7, 0)));
    assert_eq!(lab.email.sent().len(), 0);
    let stored = row(&lab.system, &id).await.unwrap();
    assert!(stored.email_sent_at().is_none());

    // The deferred run at the window's end sends it.
    let later = deliver_with_valence(&lab.delivery(), &id, utc(2026, 3, 3, 7, 0))
        .await
        .unwrap();
    assert_eq!(later.email, ChannelOutcome::Sent);
    assert_eq!(lab.email.sent().len(), 1);
}

#[tokio::test]
async fn daily_digest_skips_normal_email_but_not_urgent() {
    let lab = lab().await;
    set_settings(
        &lab.user(TEST_USER_A),
        TEST_USER_A,
        Settings {
            digest: Digest::Daily,
            ..Settings::default()
        },
    )
    .await;
    let now = utc(2026, 3, 2, 12, 0);
    let normal = send(&lab.system, TEST_USER_A, "deltest.assigned", "Normal").await;
    let urgent = send(&lab.system, TEST_USER_A, "deltest.approval", "Urgent").await;

    let r = deliver_with_valence(&lab.delivery(), &normal, now)
        .await
        .unwrap();
    assert_eq!(r.email, ChannelOutcome::Digest);
    assert_eq!(lab.email.sent().len(), 0);

    let r = deliver_with_valence(&lab.delivery(), &urgent, now)
        .await
        .unwrap();
    assert_eq!(r.email, ChannelOutcome::Sent);
    let sent = lab.email.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1.title, "Urgent");
}

#[tokio::test]
async fn channel_turned_off_before_deferred_run_sends_nothing() {
    let lab = lab().await;
    let a = lab.user(TEST_USER_A);
    set_settings(&a, TEST_USER_A, quiet_nights()).await;
    let id = send(&lab.system, TEST_USER_A, "deltest.assigned", "Held").await;

    let held = deliver_with_valence(&lab.delivery(), &id, utc(2026, 3, 2, 23, 0))
        .await
        .unwrap();
    assert!(matches!(held.email, ChannelOutcome::Held(_)));

    set_channel(
        &a,
        &user(TEST_USER_A),
        "deltest.assigned",
        Channel::Email,
        false,
    )
    .await
    .unwrap();
    let later = deliver_with_valence(&lab.delivery(), &id, utc(2026, 3, 3, 7, 0))
        .await
        .unwrap();
    assert_eq!(later.email, ChannelOutcome::Off);
    assert_eq!(lab.email.sent().len(), 0);
}

#[tokio::test]
async fn channel_failure_is_recorded_then_cleared_on_retry() {
    let lab = lab().await;
    let id = send(&lab.system, TEST_USER_A, "deltest.assigned", "Flaky").await;
    let now = utc(2026, 3, 2, 12, 0);
    lab.email.fail.store(true, Ordering::SeqCst);

    let r = deliver_with_valence(&lab.delivery(), &id, now)
        .await
        .unwrap();
    assert!(r.failed());
    let stored = row(&lab.system, &id).await.unwrap();
    assert_eq!(
        stored.delivery_error().map(String::as_str),
        Some("email failed")
    );
    assert!(stored.email_sent_at().is_none());

    lab.email.fail.store(false, Ordering::SeqCst);
    let r = deliver_with_valence(&lab.delivery(), &id, now)
        .await
        .unwrap();
    assert_eq!(r.email, ChannelOutcome::Sent);
    let stored = row(&lab.system, &id).await.unwrap();
    assert!(stored.delivery_error().is_none());
    assert!(stored.email_sent_at().is_some());
}

#[tokio::test]
async fn no_channels_installed_behaves_as_in_app_only() {
    let system = as_system(&setup_valence_with(|b| b).await);
    seed_user(TEST_USER_A, &system).await;
    assert!(!channel_configured(&system, ChannelKind::Email));
    assert!(!channel_configured(&system, ChannelKind::Text));

    let id = send(&system, TEST_USER_A, "deltest.assigned", "Plain").await;
    let delivery = system.with_actor(delivery_actor());
    let r = deliver_with_valence(&delivery, &id, Utc::now())
        .await
        .unwrap();
    assert_eq!(
        r,
        DeliveryReport {
            email: ChannelOutcome::NotConfigured,
            text: ChannelOutcome::NotConfigured,
        }
    );
    let stored = row(&system, &id).await.unwrap();
    assert!(stored.email_sent_at().is_none());
    assert!(stored.text_sent_at().is_none());
    assert!(stored.delivery_error().is_none());
    assert_eq!(digest_with_valence(&system, Utc::now()).await.unwrap(), 0);
}

#[tokio::test]
async fn verified_phone_gets_text() {
    let lab = lab().await;
    assert!(channel_configured(&lab.system, ChannelKind::Text));
    let now = utc(2026, 3, 2, 12, 0);
    verify_phone(&lab, TEST_USER_A, "+1 555 123 4567", now).await;
    let (_, phone_ok) = contact_verified(&user(TEST_USER_A), &lab.user(TEST_USER_A))
        .await
        .unwrap();
    assert!(phone_ok);

    let id = send(&lab.system, TEST_USER_A, "deltest.assigned", "Ping").await;
    let r = deliver_with_valence(&lab.delivery(), &id, now)
        .await
        .unwrap();
    assert_eq!(r.text, ChannelOutcome::Sent);
    let (to, msg) = lab.text.sent().last().cloned().unwrap();
    assert_eq!(to, Contact::Phone("+15551234567".into()));
    assert_eq!(msg.title, "Ping");
    assert!(row(&lab.system, &id)
        .await
        .unwrap()
        .text_sent_at()
        .is_some());
}

#[tokio::test]
async fn phone_code_wrong_expired_and_too_many_tries() {
    let lab = lab().await;
    let a = lab.user(TEST_USER_A);
    let me = user(TEST_USER_A);
    let t0 = utc(2026, 3, 2, 12, 0);

    assert!(matches!(
        start_phone_verification_at(&a, &me, "555-1234", t0).await,
        Err(PhoneVerificationError::InvalidNumber)
    ));
    assert!(matches!(
        confirm_phone_verification_at(&a, &me, "123456", t0).await,
        Err(PhoneVerificationError::NoPendingCode)
    ));

    // Expired after 10 minutes.
    start_phone_verification_at(&a, &me, "+15551234567", t0)
        .await
        .unwrap();
    let code = last_code(&lab.text);
    assert!(matches!(
        start_phone_verification_at(&a, &me, "+15551234567", t0 + Duration::seconds(30)).await,
        Err(PhoneVerificationError::TooSoon)
    ));
    assert!(matches!(
        confirm_phone_verification_at(&a, &me, &code, t0 + Duration::minutes(10)).await,
        Err(PhoneVerificationError::Expired)
    ));

    // Wrong code, then the fifth wrong try locks the code even when right.
    let t1 = t0 + Duration::minutes(11);
    start_phone_verification_at(&a, &me, "+15551234567", t1)
        .await
        .unwrap();
    let code = last_code(&lab.text);
    let wrong = if code == "000000" { "111111" } else { "000000" };
    for _ in 0..4 {
        assert!(matches!(
            confirm_phone_verification_at(&a, &me, wrong, t1).await,
            Err(PhoneVerificationError::WrongCode)
        ));
    }
    assert!(matches!(
        confirm_phone_verification_at(&a, &me, wrong, t1).await,
        Err(PhoneVerificationError::TooManyTries)
    ));
    assert!(matches!(
        confirm_phone_verification_at(&a, &me, &code, t1).await,
        Err(PhoneVerificationError::TooManyTries)
    ));
    let (_, phone_ok) = contact_verified(&me, &a).await.unwrap();
    assert!(!phone_ok, "nothing verified yet");

    // A fresh code works.
    let t2 = t1 + Duration::minutes(2);
    start_phone_verification_at(&a, &me, "+15551234567", t2)
        .await
        .unwrap();
    let code = last_code(&lab.text);
    confirm_phone_verification_at(&a, &me, &code, t2)
        .await
        .unwrap();
    let (_, phone_ok) = contact_verified(&me, &a).await.unwrap();
    assert!(phone_ok);
}

#[tokio::test]
async fn phone_verification_needs_a_text_channel() {
    let only_email = NotificationChannels::new().with(Recorder::new(ChannelKind::Email));
    let system =
        as_system(&setup_valence_with(|b| NotificationChannels::install(b, only_email)).await);
    seed_user(TEST_USER_A, &system).await;
    let a = as_user(&system, TEST_USER_A);
    assert!(matches!(
        start_phone_verification_at(&a, &user(TEST_USER_A), "+15551234567", Utc::now()).await,
        Err(PhoneVerificationError::TextNotConfigured)
    ));
}

#[tokio::test]
async fn contact_row_is_private_to_owner_and_delivery() {
    let lab = lab().await;
    verify_phone(&lab, TEST_USER_A, "+15551234567", utc(2026, 3, 2, 12, 0)).await;
    let purpose = valence::use_!(
        r"**Test:** Read a **Notification Contact** row in `tests` to check who may see it. CI and developers running the suite only."
    );

    let own = NotificationContact::get(TEST_USER_A, &lab.user(TEST_USER_A), purpose).await;
    assert!(own.unwrap().is_some());
    let by_delivery = NotificationContact::get(TEST_USER_A, &lab.delivery(), purpose).await;
    assert!(by_delivery.unwrap().is_some());

    let by_other = NotificationContact::get(TEST_USER_A, &lab.user(TEST_USER_B), purpose).await;
    assert!(
        !matches!(by_other, Ok(Some(_))),
        "another person can't read it"
    );
    let other_service = lab.system.with_actor(Actor::ServiceUser {
        service_name: "svc:other".into(),
    });
    let by_service = NotificationContact::get(TEST_USER_A, &other_service, purpose).await;
    assert!(
        !matches!(by_service, Ok(Some(_))),
        "another service can't read it"
    );
}

#[tokio::test]
async fn delivery_actor_cannot_create_notifications() {
    let lab = lab().await;
    let result = send_notification(
        SendNotification {
            user_id: user(TEST_USER_A),
            kind: "deltest.assigned".into(),
            title: "Forged".into(),
            ..Default::default()
        },
        &lab.delivery(),
    )
    .await;
    assert!(result.is_err());
}

/// Store a notification as System with an explicit creation and read time.
async fn seed_notification(
    v: &Valence,
    id: &str,
    to: &str,
    kind: &str,
    created_at: DateTime<Utc>,
    read_at: Option<DateTime<Utc>>,
) {
    let n = Notification::new(
        user(to),
        kind.into(),
        format!("Title {id}"),
        "Body".into(),
        None,
        None,
        read_at,
        created_at,
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    Notification::upsert(id, n, v, valence::use_!(r"**Test:** Fixture **Notification** save for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only.")).await.unwrap();
}

#[tokio::test]
async fn daily_digest_sends_once_at_eight_local() {
    let lab = lab().await;
    set_settings(
        &lab.user(TEST_USER_A),
        TEST_USER_A,
        Settings {
            digest: Digest::Daily,
            ..Settings::default()
        },
    )
    .await;
    let eight = utc(2030, 3, 5, 8, 0);
    seed_notification(
        &lab.system,
        "dg-1",
        TEST_USER_A,
        "deltest.assigned",
        eight - Duration::hours(3),
        None,
    )
    .await;
    seed_notification(
        &lab.system,
        "dg-2",
        TEST_USER_A,
        "deltest.assigned",
        eight - Duration::hours(2),
        None,
    )
    .await;
    // Read, urgent (already emailed) and too old rows stay out.
    seed_notification(
        &lab.system,
        "dg-read",
        TEST_USER_A,
        "deltest.assigned",
        eight - Duration::hours(2),
        Some(eight - Duration::hours(1)),
    )
    .await;
    seed_notification(
        &lab.system,
        "dg-urgent",
        TEST_USER_A,
        "deltest.approval",
        eight - Duration::hours(2),
        None,
    )
    .await;
    seed_notification(
        &lab.system,
        "dg-old",
        TEST_USER_A,
        "deltest.assigned",
        eight - Duration::days(2),
        None,
    )
    .await;
    // User B is on instant email and gets no digest.
    seed_notification(
        &lab.system,
        "dg-b",
        TEST_USER_B,
        "deltest.assigned",
        eight - Duration::hours(2),
        None,
    )
    .await;

    assert_eq!(
        digest_with_valence(&lab.system, eight - Duration::hours(1))
            .await
            .unwrap(),
        0
    );
    assert_eq!(digest_with_valence(&lab.system, eight).await.unwrap(), 1);
    let sent = lab.email.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0].0,
        Contact::Email(format!("{TEST_USER_A}@example.test"))
    );
    assert_eq!(sent[0].1.title, "2 unread notifications");
    assert!(sent[0].1.message.contains("Title dg-1"));
    assert!(sent[0].1.message.contains("Title dg-2"));
    for absent in ["dg-read", "dg-urgent", "dg-old", "dg-b"] {
        assert!(
            !sent[0].1.message.contains(&format!("Title {absent}")),
            "{absent}"
        );
    }

    // Not again in the same slot.
    assert_eq!(
        digest_with_valence(&lab.system, eight + Duration::minutes(30))
            .await
            .unwrap(),
        0
    );
    assert_eq!(lab.email.sent().len(), 1);
}

#[tokio::test]
async fn weekly_digest_waits_for_monday() {
    let lab = lab().await;
    set_settings(
        &lab.user(TEST_USER_A),
        TEST_USER_A,
        Settings {
            digest: Digest::Weekly,
            ..Settings::default()
        },
    )
    .await;
    // 2030-03-04 is a Monday.
    let monday = utc(2030, 3, 4, 8, 0);
    let sunday = monday - Duration::days(1);
    assert_eq!(sunday.hour(), 8);
    seed_notification(
        &lab.system,
        "wk-1",
        TEST_USER_A,
        "deltest.assigned",
        sunday - Duration::days(2),
        None,
    )
    .await;

    assert_eq!(digest_with_valence(&lab.system, sunday).await.unwrap(), 0);
    assert_eq!(digest_with_valence(&lab.system, monday).await.unwrap(), 1);
    assert_eq!(lab.email.sent()[0].1.title, "1 unread notification");
}

#[tokio::test]
async fn retention_deletes_old_read_and_unread_rows() {
    let lab = lab().await;
    let now = utc(2030, 6, 1, 3, 30);
    let s = &lab.system;
    seed_notification(
        s,
        "rt-read-old",
        TEST_USER_A,
        "deltest.assigned",
        now - Duration::days(200),
        Some(now - Duration::days(91)),
    )
    .await;
    seed_notification(
        s,
        "rt-read-new",
        TEST_USER_A,
        "deltest.assigned",
        now - Duration::days(200),
        Some(now - Duration::days(89)),
    )
    .await;
    seed_notification(
        s,
        "rt-unread-old",
        TEST_USER_A,
        "deltest.assigned",
        now - Duration::days(366),
        None,
    )
    .await;
    seed_notification(
        s,
        "rt-unread-new",
        TEST_USER_A,
        "deltest.assigned",
        now - Duration::days(300),
        None,
    )
    .await;

    assert_eq!(retention_with_valence(s, now).await.unwrap(), 2);
    assert!(row(s, "rt-read-old").await.is_none());
    assert!(row(s, "rt-unread-old").await.is_none());
    assert!(row(s, "rt-read-new").await.is_some());
    assert!(row(s, "rt-unread-new").await.is_some());
    assert_eq!(retention_with_valence(s, now).await.unwrap(), 0);
}
