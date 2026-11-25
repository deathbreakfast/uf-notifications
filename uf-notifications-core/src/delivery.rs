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
use std::str::FromStr;
use std::sync::{Arc, OnceLock};

use chrono::{DateTime, Duration, NaiveTime, TimeZone, Utc};
use chronon_coordinator::ChrononCoordinatorBackend;
use valence::{Model, Valence};

use crate::channels::{ChannelKind, NotificationChannels, OutgoingNotification};
use crate::kinds::{
    effective_channels, load_settings, verified_contact, Channels, Digest, Settings,
};
use crate::Notification;

/// What happens to one channel for one notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Send now.
    Now,
    /// Inside quiet hours; send when the window ends.
    Hold(DateTime<Utc>),
    /// Email under a digest; the digest picks it up.
    Digest,
    /// The channel is off for this kind and person.
    Off,
}

/// Email and text [`Step`]s for one notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Route {
    /// Email.
    pub email: Step,
    /// Text.
    pub text: Step,
}

fn parse_hh_mm(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s, "%H:%M").ok()
}

/// The person's time zone; an unknown name falls back to UTC.
pub(crate) fn settings_tz(settings: &Settings) -> chrono_tz::Tz {
    chrono_tz::Tz::from_str(settings.time_zone.trim()).unwrap_or(chrono_tz::UTC)
}

/// The UTC instant of local `date` + `time` in `tz`. In a DST gap, the first
/// valid time after it.
fn local_instant(
    tz: chrono_tz::Tz,
    date: chrono::NaiveDate,
    time: NaiveTime,
) -> Option<DateTime<Utc>> {
    let naive = date.and_time(time);
    (0..=3)
        .find_map(|h| {
            tz.from_local_datetime(&(naive + Duration::hours(h)))
                .earliest()
        })
        .map(|dt| dt.with_timezone(&Utc))
}

/// When `now` is inside the person's quiet hours, the instant they end.
///
/// The window may cross midnight (22:00–07:00). Quiet hours that are off,
/// that start and end at the same time, or that don't parse are never in
/// effect. The times are local to the settings' time zone.
pub fn quiet_hours_end(settings: &Settings, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    if !settings.quiet_enabled {
        return None;
    }
    let start = parse_hh_mm(&settings.quiet_start)?;
    let end = parse_hh_mm(&settings.quiet_end)?;
    if start == end {
        return None;
    }
    let tz = settings_tz(settings);
    let local = now.with_timezone(&tz);
    let t = local.time();
    let inside = if start < end {
        start <= t && t < end
    } else {
        t >= start || t < end
    };
    if !inside {
        return None;
    }
    let today = local.date_naive();
    let end_date = if t < end { today } else { today.succ_opt()? };
    local_instant(tz, end_date, end)
}

/// Decide each channel for one notification.
///
/// `channels` are the person's [effective channels](crate::effective_channels)
/// for the kind. Quiet hours hold email and text, urgent or not. A daily or
/// weekly digest takes email for every kind that isn't `urgent`; text is never
/// part of a digest.
pub fn route(settings: &Settings, channels: Channels, urgent: bool, now: DateTime<Utc>) -> Route {
    let hold = quiet_hours_end(settings, now);
    let email = if !channels.email {
        Step::Off
    } else if settings.digest != Digest::Instant && !urgent {
        Step::Digest
    } else {
        hold.map_or(Step::Now, Step::Hold)
    };
    let text = if channels.text {
        hold.map_or(Step::Now, Step::Hold)
    } else {
        Step::Off
    };
    Route { email, text }
}

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
    let errors: Vec<String> = [("email", &report.email), ("text", &report.text)]
        .into_iter()
        .filter_map(|(name, o)| match o {
            ChannelOutcome::Failed(e) => Some(format!("{name}: {e}")),
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
        Err(_) => return,
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
        NotificationsDeliverLaterScript::scheduler(backend, valence.clone())
            .name(format!("notifications.deliver_later.{notification_id}"))
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

    fn quiet(start: &str, end: &str, tz: &str) -> Settings {
        Settings {
            quiet_enabled: true,
            quiet_start: start.into(),
            quiet_end: end.into(),
            time_zone: tz.into(),
            ..Settings::default()
        }
    }

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .expect("valid")
    }

    const ALL: Channels = Channels {
        in_app: true,
        email: true,
        text: true,
    };

    #[test]
    fn quiet_window_crossing_midnight_ends_next_morning() {
        let s = quiet("22:00", "07:00", "UTC");
        assert_eq!(
            quiet_hours_end(&s, utc(2026, 3, 2, 23, 30)),
            Some(utc(2026, 3, 3, 7, 0))
        );
        assert_eq!(
            quiet_hours_end(&s, utc(2026, 3, 3, 6, 59)),
            Some(utc(2026, 3, 3, 7, 0))
        );
        assert_eq!(quiet_hours_end(&s, utc(2026, 3, 3, 7, 0)), None);
        assert_eq!(quiet_hours_end(&s, utc(2026, 3, 3, 12, 0)), None);
    }

    #[test]
    fn quiet_window_same_day_and_time_zone() {
        // 13:00–14:00 in New York (UTC-5 in January) is 18:00–19:00 UTC.
        let s = quiet("13:00", "14:00", "America/New_York");
        assert_eq!(
            quiet_hours_end(&s, utc(2026, 1, 5, 18, 15)),
            Some(utc(2026, 1, 5, 19, 0))
        );
        assert_eq!(quiet_hours_end(&s, utc(2026, 1, 5, 13, 15)), None);
    }

    #[test]
    fn quiet_window_off_empty_or_bad_zone() {
        let mut s = quiet("22:00", "07:00", "Not/AZone");
        assert_eq!(
            quiet_hours_end(&s, utc(2026, 3, 2, 23, 0)),
            Some(utc(2026, 3, 3, 7, 0))
        );
        s.quiet_enabled = false;
        assert_eq!(quiet_hours_end(&s, utc(2026, 3, 2, 23, 0)), None);
        let same = quiet("09:00", "09:00", "UTC");
        assert_eq!(quiet_hours_end(&same, utc(2026, 3, 2, 9, 0)), None);
    }

    #[test]
    fn quiet_window_ending_in_dst_gap_moves_forward() {
        // 2026-03-08 02:30 doesn't exist in New York; the window ends at 03:30 EDT.
        let s = quiet("01:00", "02:30", "America/New_York");
        let end = quiet_hours_end(&s, utc(2026, 3, 8, 6, 30)).expect("inside");
        assert_eq!(end, utc(2026, 3, 8, 7, 30));
    }

    #[test]
    fn route_holds_both_channels_in_quiet_hours() {
        let s = quiet("22:00", "07:00", "UTC");
        let r = route(&s, ALL, false, utc(2026, 3, 2, 23, 0));
        let at = utc(2026, 3, 3, 7, 0);
        assert_eq!(
            r,
            Route {
                email: Step::Hold(at),
                text: Step::Hold(at)
            }
        );
    }

    #[test]
    fn route_digest_takes_email_unless_urgent() {
        let s = Settings {
            digest: Digest::Daily,
            ..Settings::default()
        };
        let now = utc(2026, 3, 2, 12, 0);
        assert_eq!(
            route(&s, ALL, false, now),
            Route {
                email: Step::Digest,
                text: Step::Now
            }
        );
        assert_eq!(
            route(&s, ALL, true, now),
            Route {
                email: Step::Now,
                text: Step::Now
            }
        );
    }

    #[test]
    fn route_off_channels_stay_off() {
        let r = route(
            &Settings::default(),
            Channels::IN_APP_ONLY,
            true,
            utc(2026, 3, 2, 12, 0),
        );
        assert_eq!(
            r,
            Route {
                email: Step::Off,
                text: Step::Off
            }
        );
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
