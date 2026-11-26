//! Email and text delivery for one notification.
//!
//! [`send_notification`](crate::send_notification) writes the in-app row and,
//! when the host installed a [channel](crate::channels), enqueues the Boson
//! task `notifications.deliver` as the
//! [delivery actor](crate::delivery_policy::delivery_actor). The task calls
//! [`deliver_with_valence`], which re-reads the recipient's preferences and
//! settings and, per channel:
//!
//! - inside quiet hours, holds the message until the window ends: the task
//!   schedules the Chronon run-once job `notifications.deliver_later` for
//!   that time through the backend the host passed to
//!   [`register_delivery_scheduler`];
//! - for email under a daily or weekly digest and a kind that isn't
//!   [`urgent`](crate::NotificationKind::urgent), leaves it to the
//!   [digest](crate::digest);
//! - otherwise sends now to the verified contact.
//!
//! The deferred job runs the same check again, so a channel turned off in
//! between sends nothing. Each run records `email_sent_at`, `text_sent_at`
//! or `delivery_error` on the notification. A channel already sent is never
//! sent twice, so Boson's retries after a failure are safe.

use std::fmt;
use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Utc};
use chronon_coordinator::ChrononCoordinatorBackend;
use valence::{Model, Valence};

use crate::channels::{ChannelKind, NotificationChannels, OutgoingNotification};
use crate::kinds::{effective_channels, load_settings, verified_contact};
use crate::Notification;

pub use crate::routing::{quiet_hours_end, route, Route, Step};

/// What one delivery run did with one channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelOutcome {
    /// Delivered in this run.
    Sent,
    /// Delivered by an earlier run.
    AlreadySent,
    /// Held for quiet hours until this instant.
    Held(DateTime<Utc>),
    /// Left to the digest.
    Digest,
    /// Off for this kind and person.
    Off,
    /// The host installed no channel of this kind.
    NotConfigured,
    /// On, but no verified address or number to send to.
    NoContact,
    /// The channel failed; the text is recorded on the row.
    Failed(String),
}

/// What one delivery run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryReport {
    /// Email.
    pub email: ChannelOutcome,
    /// Text.
    pub text: ChannelOutcome,
}

impl DeliveryReport {
    const NOTHING: Self = Self {
        email: ChannelOutcome::NotConfigured,
        text: ChannelOutcome::NotConfigured,
    };

    /// The earliest instant a channel is held until, if any.
    pub fn held_until(&self) -> Option<DateTime<Utc>> {
        [&self.email, &self.text]
            .into_iter()
            .filter_map(|o| match o {
                ChannelOutcome::Held(at) => Some(*at),
                _ => None,
            })
            .min()
    }

    /// Whether a channel failed in this run.
    pub fn failed(&self) -> bool {
        matches!(self.email, ChannelOutcome::Failed(_))
            || matches!(self.text, ChannelOutcome::Failed(_))
    }
}

/// Delivery failed before any channel ran.
#[derive(Debug)]
pub enum DeliveryError {
    /// Valence refused or failed a read or write.
    Store(valence::Error),
    /// A channel failed; the run recorded it and should be retried.
    Channel(String),
    /// The deferred run could not be scheduled.
    Schedule(String),
}

impl fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(err) => write!(f, "notification delivery store: {err}"),
            Self::Channel(err) => write!(f, "notification delivery channel: {err}"),
            Self::Schedule(err) => write!(f, "notification delivery schedule: {err}"),
        }
    }
}

impl std::error::Error for DeliveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(err) => Some(err),
            _ => None,
        }
    }
}

impl From<valence::Error> for DeliveryError {
    fn from(err: valence::Error) -> Self {
        Self::Store(err)
    }
}

/// The message a channel sends for `row`.
fn outgoing(row: &Notification, id: &str) -> OutgoingNotification {
    OutgoingNotification {
        title: row.title().clone(),
        message: row.message().clone(),
        url: crate::sanitize_notification_url(row.url().cloned()),
        kind_id: row.kind().clone(),
        notification_id: Some(id.to_string()),
    }
}

/// Run one channel's [`Step`].
async fn run_step(
    step: Step,
    kind: ChannelKind,
    already_sent: bool,
    row: &Notification,
    id: &str,
    channels: &NotificationChannels,
    valence: &Valence,
) -> valence::Result<ChannelOutcome> {
    if already_sent {
        return Ok(ChannelOutcome::AlreadySent);
    }
    match step {
        Step::Off => Ok(ChannelOutcome::Off),
        Step::Digest => Ok(ChannelOutcome::Digest),
        Step::Hold(at) => Ok(ChannelOutcome::Held(at)),
        Step::Now => {
            let Some(channel) = channels.channel(kind) else {
                return Ok(ChannelOutcome::NotConfigured);
            };
            let Some(contact) = verified_contact(row.user(), valence, kind).await? else {
                return Ok(ChannelOutcome::NoContact);
            };
            Ok(match channel.deliver(&contact, &outgoing(row, id)).await {
                Ok(()) => ChannelOutcome::Sent,
                Err(err) => ChannelOutcome::Failed(err.to_string()),
            })
        }
    }
}

/// Deliver notification `notification_id` by email and text as its
/// recipient's preferences say, at `now`.
///
/// Run as the [delivery actor](crate::delivery_policy::delivery_actor). With
/// no channels installed, or a row that no longer exists, nothing happens.
/// Records `email_sent_at` / `text_sent_at` for what was sent and
/// `delivery_error` for what failed (cleared when nothing failed).
///
/// # Errors
///
/// Returns the Valence error when a read or the row update fails. Channel
/// failures are reported in the [`DeliveryReport`], not as errors.
pub async fn deliver_with_valence(
    valence: &Valence,
    notification_id: &str,
    now: DateTime<Utc>,
) -> valence::Result<DeliveryReport> {
    let Some(channels) = NotificationChannels::get(valence).filter(|c| !c.is_empty()) else {
        return Ok(DeliveryReport::NOTHING);
    };
    let Some(row) = Notification::get(notification_id, valence, valence::use_!(r"When **notification delivery** sends one notification by email or text, we **read Notification** for its title, message, link and recipient, and whether it already went out. Only the notification delivery service reads it this way.")).await? else {
        return Ok(DeliveryReport {
            email: ChannelOutcome::Off,
            text: ChannelOutcome::Off,
        });
    };
    let user = row.user().clone();
    let urgent = crate::kinds::kind(row.kind()).is_some_and(|k| k.urgent);
    let chosen = effective_channels(valence, &user, row.kind()).await?;
    let settings = load_settings(valence, &user).await?;
    let plan = route(&settings, chosen, urgent, now);

    let email = run_step(
        plan.email,
        ChannelKind::Email,
        row.email_sent_at().is_some(),
        &row,
        notification_id,
        &channels,
        valence,
    )
    .await?;
    let text = run_step(
        plan.text,
        ChannelKind::Text,
        row.text_sent_at().is_some(),
        &row,
        notification_id,
        &channels,
        valence,
    )
    .await?;
    let report = DeliveryReport { email, text };
    record(valence, row, &report, now).await?;
    Ok(report)
}

/// Write what `report` sent or failed onto `row`.
async fn record(
    valence: &Valence,
    row: Notification,
    report: &DeliveryReport,
    now: DateTime<Utc>,
) -> valence::Result<()> {
    let sent_email = report.email == ChannelOutcome::Sent;
    let sent_text = report.text == ChannelOutcome::Sent;
    // Store a fixed category on the row the recipient can read; provider
    // text can carry endpoints, account ids or other people's details, so it
    // only goes to tracing.
    let errors: Vec<&str> = [("email", &report.email), ("text", &report.text)]
        .into_iter()
        .filter_map(|(name, o)| match o {
            ChannelOutcome::Failed(detail) => {
                tracing::warn!(operation = "deliver", outcome = "channel_err", channel = name, error = %detail);
                Some(if name == "email" { "email failed" } else { "text failed" })
            }
            _ => None,
        })
        .collect();
    if !sent_email && !sent_text && errors.is_empty() {
        return Ok(());
    }
    let mut m = row.get_mutable(valence, valence::use_!(r"When **notification delivery** sends a notification by email or text, we **update Notification** with when each went out or why it failed, so it isn't sent twice and failures can be retried. Only the notification delivery service writes these fields."));
    if sent_email {
        m = m.set_email_sent_at(now)?;
    }
    if sent_text {
        m = m.set_text_sent_at(now)?;
    }
    m = if errors.is_empty() {
        m.clear_delivery_error()
    } else {
        m.set_delivery_error(errors.join("; "))?
    };
    m.commit().await?;
    Ok(())
}

/// Note on the row that a held message can't be sent later.
async fn record_unscheduled(valence: &Valence, notification_id: &str) -> valence::Result<()> {
    let Some(row) = Notification::get(notification_id, valence, valence::use_!(r"When **notification delivery** can't hold an email or text until quiet hours end, we **read Notification** to record why it wasn't sent. Only the notification delivery service reads it this way.")).await? else {
        return Ok(());
    };
    row.get_mutable(valence, valence::use_!(r"When **notification delivery** can't hold an email or text until quiet hours end, we **update Notification** with why it wasn't sent. Only the notification delivery service writes this field."))
        .set_delivery_error(UNSCHEDULED.to_string())?
        .commit()
        .await?;
    Ok(())
}

/// `delivery_error` when quiet hours held a message and no scheduler is registered.
pub const UNSCHEDULED: &str = "deferred delivery unavailable";

static DELIVERY_SCHEDULER: OnceLock<Arc<dyn ChrononCoordinatorBackend>> = OnceLock::new();

/// [`register_delivery_scheduler`] was called a second time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeliverySchedulerAlreadyRegistered;

impl fmt::Display for DeliverySchedulerAlreadyRegistered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a notification delivery scheduler is already registered")
    }
}

impl std::error::Error for DeliverySchedulerAlreadyRegistered {}

/// Register the host's Chronon backend once at boot so email and text held
/// for quiet hours go out when the window ends.
///
/// Without one, a held message records [`UNSCHEDULED`] on the notification
/// and is not sent.
///
/// # Errors
///
/// [`DeliverySchedulerAlreadyRegistered`] when a backend is already set; the
/// first registration stays.
pub fn register_delivery_scheduler(
    backend: Arc<dyn ChrononCoordinatorBackend>,
) -> Result<(), DeliverySchedulerAlreadyRegistered> {
    DELIVERY_SCHEDULER
        .set(backend)
        .map_err(|_| DeliverySchedulerAlreadyRegistered)
}

/// Deliver now and, for a message held by quiet hours, schedule the deferred
/// run. Shared by the Boson task and the Chronon script.
async fn deliver_and_schedule(
    valence: &Valence,
    notification_id: &str,
) -> Result<DeliveryReport, DeliveryError> {
    let report = deliver_with_valence(valence, notification_id, Utc::now()).await?;
    if let Some(at) = report.held_until() {
        match DELIVERY_SCHEDULER.get() {
            Some(backend) => {
                jobs::schedule_deliver_later(backend.as_ref(), valence, notification_id, at)
                    .await
                    .map_err(DeliveryError::Schedule)?
            }
            None => {
                tracing::warn!(
                    operation = "deliver",
                    outcome = "unscheduled",
                    "quiet hours held a notification but no delivery scheduler is registered"
                );
                record_unscheduled(valence, notification_id).await?;
            }
        }
    }
    if report.failed() {
        let why = [&report.email, &report.text]
            .into_iter()
            .find_map(|o| match o {
                ChannelOutcome::Failed(e) => Some(e.clone()),
                _ => None,
            })
            .unwrap_or_default();
        return Err(DeliveryError::Channel(why));
    }
    Ok(report)
}

/// Enqueue `notifications.deliver` for `notification_id` as the delivery
/// actor, when `valence` has channels installed. Never fails the send: an
/// enqueue error is logged.
pub(crate) async fn enqueue_delivery(valence: &Valence, notification_id: &str) {
    if NotificationChannels::get(valence).is_none_or(|c| c.is_empty()) {
        return;
    }
    let actor = match serde_json::to_value(crate::delivery_policy::delivery_actor()) {
        Ok(actor) => actor,
        Err(err) => {
            tracing::warn!(
                operation = "send",
                outcome = "enqueue_err",
                error = %err,
                "could not serialize the delivery actor; email and text not enqueued"
            );
            return;
        }
    };
    if let Err(err) = jobs::NotificationsDeliver::send_with(
        actor,
        jobs::NotificationsDeliverParams {
            notification_id: notification_id.to_string(),
        },
    )
    .await
    {
        tracing::warn!(
            operation = "send",
            outcome = "enqueue_err",
            error = %err,
            "could not enqueue email and text delivery"
        );
    }
}

/// The Boson task and Chronon script; thin wrappers over
/// [`deliver_and_schedule`].
mod jobs {
    #![allow(dead_code, missing_docs, clippy::pedantic, clippy::nursery)]

    use chrono::{DateTime, Utc};
    use chronon_coordinator::{ChrononCoordinatorBackend, RetryPolicy};
    use valence::Valence;

    #[boson_macros::task(
        name = "notifications.deliver",
        max_attempts = 5,
        base_delay_ms = 30_000,
        backoff_multiplier = 2.0,
        max_delay_ms = 900_000
    )]
    pub(crate) async fn notifications_deliver(
        ctx: Box<dyn boson_core::ExecutionContext>,
        notification_id: String,
    ) -> boson_core::Result<()> {
        let v = boson_valence_identity::valence_from_context(ctx.as_ref())?;
        super::deliver_and_schedule(&v, &notification_id)
            .await
            .map(drop)
            .map_err(|e| boson_core::BosonError::internal(e.to_string()))
    }

    #[chronon_coordinator_macros::script(name = "notifications.deliver_later")]
    pub(crate) async fn notifications_deliver_later(
        ctx: Box<dyn chronon_core::ScriptContext>,
        notification_id: String,
    ) -> anyhow::Result<()> {
        let v = chronon_valence_identity::valence_from_context(&*ctx)?;
        super::deliver_and_schedule(&v, &notification_id)
            .await
            .map(drop)
            .map_err(anyhow::Error::from)
    }

    pub(super) async fn schedule_deliver_later(
        backend: &dyn ChrononCoordinatorBackend,
        valence: &Valence,
        notification_id: &str,
        at: DateTime<Utc>,
    ) -> Result<(), String> {
        let name = format!("notifications.deliver_later.{notification_id}");
        // A Boson retry after the other channel failed lands here again; the
        // job from the first attempt is already queued.
        if NotificationsDeliverLaterScript::get_job_by_name(backend, &name)
            .await
            .is_ok()
        {
            return Ok(());
        }
        NotificationsDeliverLaterScript::scheduler(backend, valence.clone())
            .name(name)
            .run_once_at(at)
            .params(NotificationsDeliverLaterParams {
                notification_id: notification_id.to_string(),
            })
            .retry_policy(RetryPolicy {
                max_attempts: 5,
                base_delay_ms: 30_000,
                backoff_multiplier: 2.0,
                max_delay_ms: 900_000,
            })
            .add()
            .await
            .map(drop)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .expect("valid")
    }

    #[test]
    fn report_held_until_and_failed() {
        let at = utc(2026, 3, 3, 7, 0);
        let r = DeliveryReport {
            email: ChannelOutcome::Held(at),
            text: ChannelOutcome::Failed("boom".into()),
        };
        assert_eq!(r.held_until(), Some(at));
        assert!(r.failed());
        assert_eq!(DeliveryReport::NOTHING.held_until(), None);
        assert!(!DeliveryReport::NOTHING.failed());
    }
}
