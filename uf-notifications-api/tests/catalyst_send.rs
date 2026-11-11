//! `notifications.send` against a real Valence: who is notified, who is skipped,
//! and who the sender is.

#![cfg(feature = "catalyst")]
#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use ::catalyst::people::TeamMembers;
use ::catalyst::registry::{ActionKind, ExecContext, RuleRunRef, RunAs, SubjectRef};
use ::catalyst::CatalystError;
use chrono::Utc;
use common::{as_system, as_user, seed_user, setup_shared_db, TEST_USER_A, TEST_USER_B};
use serde_json::{json, Value};
use uf_notifications_api::catalyst::{NotifyActor, NotifySend};
use uf_notifications_core::{register_notification_audience, Notification};
use uuid::Uuid;
use valence::{Actor, Model, RecordId, RecordPredicate, Valence};

const BOT: &str = "svc:notify-bot";
/// Recipient whose audience check errors, standing in for a storage fault.
const BROKEN: &str = "notify-test-broken";
const OUTSIDER: &str = "notify-test-outsider";

/// Audience for catalyst notifications: users A and B only, whoever sends.
fn register_audience() {
    // `notification` rows are readable only by their owner; `user` rows by any
    // authenticated actor, which is what a bot's subject has to be.
    for table in ["notification", "user"] {
        register_notification_audience(table, Some("catalyst"), |_subject, recipient, _v| {
            let admitted = recipient == TEST_USER_A || recipient == TEST_USER_B;
            let broken = recipient == BROKEN;
            Box::pin(async move {
                if broken {
                    return Err(valence::Error::Internal("audience store down".into()));
                }
                Ok(admitted)
            })
        });
    }
}

/// Hands out the owner's handle, or the bot's when the fire runs as the bot.
struct Actors {
    owner: Valence,
    bot: Valence,
    seen: Arc<Mutex<Vec<RunAs>>>,
}

impl NotifyActor for Actors {
    fn valence_for<'a>(
        &'a self,
        ctx: &'a ExecContext,
    ) -> Pin<Box<dyn Future<Output = Result<Valence, CatalystError>> + Send + 'a>> {
        self.seen.lock().unwrap().push(ctx.run_as().clone());
        let v = match ctx.run_as() {
            RunAs::Bot { .. } => self.bot.clone(),
            _ => self.owner.clone(),
        };
        Box::pin(async move { Ok(v) })
    }
}

struct Failing;

impl NotifyActor for Failing {
    fn valence_for<'a>(
        &'a self,
        _ctx: &'a ExecContext,
    ) -> Pin<Box<dyn Future<Output = Result<Valence, CatalystError>> + Send + 'a>> {
        Box::pin(async { Err(CatalystError::internal("actor.unavailable")) })
    }
}

struct Teams;

impl TeamMembers for Teams {
    fn members<'a>(
        &'a self,
        _ctx: &'a ExecContext,
        team_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, CatalystError>> + Send + 'a>> {
        let members = match team_id {
            "t-both" => Ok(vec![TEST_USER_A.to_string(), TEST_USER_B.to_string()]),
            _ => Err(CatalystError::internal("teams.members")),
        };
        Box::pin(async move { members })
    }
}

struct Fixture {
    base: Valence,
    action: NotifySend<Actors>,
    seen: Arc<Mutex<Vec<RunAs>>>,
}

async fn fixture() -> Fixture {
    register_audience();
    let base = setup_shared_db().await;
    seed_user(OUTSIDER, &base).await;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let actors = Actors {
        owner: as_user(&base, TEST_USER_A),
        bot: base.with_actor(Actor::ServiceUser {
            service_name: BOT.to_string(),
        }),
        seen: seen.clone(),
    };
    Fixture {
        base,
        action: NotifySend::new(actors),
        seen,
    }
}

/// Notification row owned by `owner`, so only `owner` (and System) can read it.
async fn subject_owned_by(system: &Valence, owner: &str) -> SubjectRef {
    let id = Uuid::new_v4().to_string();
    let row = Notification::new(
        RecordId::new("user", owner),
        "test".into(),
        "Subject".into(),
        "body".into(),
        None,
        None,
        None,
        Utc::now(),
        None,
        None,
    )
    .expect("construct notification");
    Notification::upsert(&id, row, system, valence::use_!(r"**Test:** Fixture **Notification** save for `catalyst_send` so the suite can arrange and assert persistence behavior. CI and developers running the suite only."))
        .await
        .expect("system may create");
    SubjectRef::new("notification", id)
}

async fn run(
    f: &Fixture,
    ctx: &ExecContext,
    params: &Value,
    subject: &SubjectRef,
) -> Result<catalyst::registry::ActionOutcome, CatalystError> {
    f.action
        .execute(ctx, params, subject, &RuleRunRef::new("run-1"))
        .await
}

/// Notifications `user` holds, read as System.
async fn inbox(f: &Fixture, user: &str) -> Vec<Notification> {
    Notification::query(&as_system(&f.base), valence::use_!(r"**Test:** Fixture **Notification** load for `catalyst_send` so the suite can arrange and assert persistence behavior. CI and developers running the suite only."))
        .where_user(RecordPredicate::Equals(RecordId::new("user", user)))
        .limit(50)
        .await
        .expect("query inbox")
        .into_iter()
        .filter(|n| n.kind() == "catalyst")
        .collect()
}

#[tokio::test]
async fn person_is_notified_happy() {
    let f = fixture().await;
    let subject = subject_owned_by(&as_system(&f.base), TEST_USER_A).await;
    let params =
        json!({"to": [{"user": TEST_USER_B}], "text": "Build is red", "link": "/builds/7"});

    let out = run(&f, &ExecContext::new(), &params, &subject)
        .await
        .expect("send");

    assert_eq!(out.status, "ok");
    assert_eq!(out.outputs["sent"], 1);
    let rows = inbox(&f, TEST_USER_B).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].message(), "Build is red");
    assert_eq!(rows[0].url().map(String::as_str), Some("/builds/7"));
    assert_eq!(rows[0].sender().map(|s| s.id()), Some(TEST_USER_A));
}

#[tokio::test]
async fn team_expands_and_dedupes_members_happy() {
    let f = fixture().await;
    let subject = subject_owned_by(&as_system(&f.base), TEST_USER_A).await;
    let ctx = ExecContext::new().with_team_members(Some(Arc::new(Teams)));
    let params = json!({"to": [{"team": "t-both"}, {"user": TEST_USER_B}], "text": "Standup"});

    let out = run(&f, &ctx, &params, &subject).await.expect("send");

    assert_eq!(out.outputs["sent"], 2, "A and B once each");
    assert_eq!(inbox(&f, TEST_USER_A).await.len(), 1);
    assert_eq!(inbox(&f, TEST_USER_B).await.len(), 1);
}

#[tokio::test]
async fn team_without_hook_fails_and_sends_nothing_sad() {
    let f = fixture().await;
    let subject = subject_owned_by(&as_system(&f.base), TEST_USER_A).await;
    let params = json!({"to": [{"team": "t-both"}], "text": "Standup"});

    let err = run(&f, &ExecContext::new(), &params, &subject)
        .await
        .expect_err("no team hook");

    assert!(matches!(err, CatalystError::KindFailed { .. }));
    assert!(inbox(&f, TEST_USER_A).await.is_empty());
}

#[tokio::test]
async fn recipient_outside_audience_is_skipped_and_counted_sad() {
    let f = fixture().await;
    let subject = subject_owned_by(&as_system(&f.base), TEST_USER_A).await;
    let params = json!({
        "to": [{"user": TEST_USER_B}, {"user": OUTSIDER}],
        "text": "Heads up"
    });

    let out = run(&f, &ExecContext::new(), &params, &subject)
        .await
        .expect("a skipped recipient does not fail the action");

    assert_eq!(out.status, "partial");
    assert_eq!(out.outputs, json!({"sent": 1, "skipped": 1}));
    assert_eq!(inbox(&f, TEST_USER_B).await.len(), 1);
    assert!(inbox(&f, OUTSIDER).await.is_empty());
}

#[tokio::test]
async fn unreadable_subject_skips_everyone_sad() {
    let f = fixture().await;
    // Owned by B, so the owner actor (A) cannot read it.
    let subject = subject_owned_by(&as_system(&f.base), TEST_USER_B).await;
    let params = json!({"to": [{"user": TEST_USER_A}, {"user": TEST_USER_B}], "text": "Hi"});

    let out = run(&f, &ExecContext::new(), &params, &subject)
        .await
        .expect("denials are skips");

    assert_eq!(out.status, "partial");
    assert_eq!(out.outputs, json!({"sent": 0, "skipped": 2}));
}

#[tokio::test]
async fn run_as_bot_sends_as_the_bot_happy() {
    let f = fixture().await;
    // A user record any authenticated actor, bots included, can read.
    let subject = SubjectRef::new("user", TEST_USER_A);
    let ctx = ExecContext::new().with_run_as(RunAs::Bot {
        bot_user_id: BOT.to_string(),
    });
    let params = json!({"to": [{"user": TEST_USER_B}], "text": "From the bot"});

    let out = run(&f, &ctx, &params, &subject).await.expect("send");

    assert_eq!(out.outputs["sent"], 1);
    let rows = inbox(&f, TEST_USER_B).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].sender().map(|s| s.id()), Some(BOT));
    assert!(matches!(
        f.seen.lock().unwrap().as_slice(),
        [RunAs::Bot { .. }]
    ));
}

#[tokio::test]
async fn run_as_owner_does_not_use_the_bot_sad() {
    let f = fixture().await;
    let subject = subject_owned_by(&as_system(&f.base), BOT).await;
    let params = json!({"to": [{"user": TEST_USER_B}], "text": "From the owner"});

    // Owner (A) cannot read the bot's row, so the same fire as owner is skipped.
    let out = run(&f, &ExecContext::new(), &params, &subject)
        .await
        .expect("denial is a skip");

    assert_eq!(out.outputs, json!({"sent": 0, "skipped": 1}));
    assert!(matches!(f.seen.lock().unwrap().as_slice(), [RunAs::Owner]));
    assert!(inbox(&f, TEST_USER_B).await.is_empty());
}

#[tokio::test]
async fn actor_failure_fails_the_action_sad() {
    let f = fixture().await;
    let subject = subject_owned_by(&as_system(&f.base), TEST_USER_A).await;
    let action = NotifySend::new(Failing);
    let params = json!({"to": [{"user": TEST_USER_B}], "text": "Hi"});

    let err = action
        .execute(
            &ExecContext::new(),
            &params,
            &subject,
            &RuleRunRef::new("r"),
        )
        .await
        .expect_err("actor unavailable");

    assert!(matches!(err, CatalystError::Internal { .. }));
    assert!(inbox(&f, TEST_USER_B).await.is_empty());
}

#[tokio::test]
async fn non_privacy_send_failure_keeps_its_reason_sad() {
    let f = fixture().await;
    let subject = subject_owned_by(&as_system(&f.base), TEST_USER_A).await;
    let params = json!({"to": [{"user": TEST_USER_B}, {"user": BROKEN}], "text": "Hi"});

    let err = run(&f, &ExecContext::new(), &params, &subject)
        .await
        .expect_err("a non-privacy failure fails the action");

    let CatalystError::KindFailed { reason, .. } = &err else {
        panic!("expected KindFailed, got {err:?}");
    };
    assert_eq!(*reason, "persist_failed");
}

#[tokio::test]
async fn simulate_lists_recipients_and_writes_nothing_happy() {
    let f = fixture().await;
    let subject = subject_owned_by(&as_system(&f.base), TEST_USER_A).await;
    let ctx = ExecContext::new().with_team_members(Some(Arc::new(Teams)));
    let params = json!({"to": [{"team": "t-both"}], "text": "Standup"});

    let sim = f
        .action
        .simulate(&ctx, &params, &subject, Utc::now())
        .await
        .expect("simulate");

    assert_eq!(
        sim.status,
        format!("would_notify:{TEST_USER_A},{TEST_USER_B}")
    );
    assert!(inbox(&f, TEST_USER_A).await.is_empty());
    assert!(inbox(&f, TEST_USER_B).await.is_empty());
    assert!(f.seen.lock().unwrap().is_empty(), "simulate needs no actor");
}
