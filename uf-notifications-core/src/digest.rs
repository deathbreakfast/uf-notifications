//! Daily and weekly digest email.
//!
//! The Chronon script `notifications.digest` runs every hour (default job
//! `notifications-digest`, cron `0 * * * *`). The host registers default jobs
//! with a System Valence, so the job starts as System; nothing elevates. For
//! each person on a daily digest whose local time is 8 AM, and each person on
//! a weekly digest whose local time is Monday 8 AM, it emails one message
//! listing unread notifications since their last digest, for kinds with
//! email on that aren't [`urgent`](crate::NotificationKind::urgent) (those
//! went out right away). When the digest is sent, or there was nothing to
//! send, it records `last_digest_at` so the slot isn't sent twice.

use std::collections::HashMap;

use chrono::{DateTime, Datelike, Duration, Timelike, Utc, Weekday};
use valence::{DateTimePredicate, RecordId, RecordPredicate, SortDirection, Valence};

use crate::channels::{ChannelKind, NotificationChannels, OutgoingNotification, DIGEST_KIND_ID};
use crate::generated::{NotificationSettings, NotificationSettingsDigest};
use crate::kinds::{effective_channels, verified_contact, Digest, Settings};
use crate::routing::settings_tz;
use crate::Notification;

/// Local hour the digest goes out.
pub const DIGEST_HOUR: u32 = 8;
/// Most notifications one digest lists.
pub const MAX_DIGEST_ITEMS: u32 = 50;

/// Whether `now` is a digest slot for `settings`, and the slot wasn't sent
/// yet (`last` is in an earlier local day).
pub fn digest_due(settings: &Settings, last: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    let tz = settings_tz(settings);
    let local = now.with_timezone(&tz);
    let slot = match settings.digest {
        Digest::Instant => false,
        Digest::Daily => local.hour() == DIGEST_HOUR,
        Digest::Weekly => local.hour() == DIGEST_HOUR && local.weekday() == Weekday::Mon,
    };
    slot && last.is_none_or(|l| l.with_timezone(&tz).date_naive() < local.date_naive())
}

fn settings_of(row: &NotificationSettings) -> Settings {
    Settings {
        digest: row.digest().into(),
        quiet_enabled: *row.quiet_enabled(),
        quiet_start: row.quiet_start().clone(),
        quiet_end: row.quiet_end().clone(),
        time_zone: row.time_zone().clone(),
    }
}

/// The digest body: one line per notification, oldest first.
fn digest_message(rows: &[Notification]) -> OutgoingNotification {
    let lines: Vec<String> = rows
        .iter()
        .map(|n| format!("- {}: {}", n.title(), n.message()))
        .collect();
    OutgoingNotification {
        title: if rows.len() == 1 {
            "1 unread notification".to_string()
        } else {
            format!("{} unread notifications", rows.len())
        },
        message: lines.join("\n"),
        url: Some("/notifications".to_string()),
        kind_id: DIGEST_KIND_ID.to_string(),
        notification_id: None,
    }
}

/// `user`'s unread notifications created after `since` that belong in a
/// digest, oldest first.
async fn digest_rows(
    valence: &Valence,
    user: &RecordId,
    since: DateTime<Utc>,
) -> valence::Result<Vec<Notification>> {
    let unread: Vec<Notification> = Notification::query(valence, valence::use_!(r"When the **notification digest** job builds one person's daily or weekly email, we **list Notification** rows they haven't read since their last digest, to list their titles. Only the digest job reads them this way, and only to email that person."))
        .where_user(RecordPredicate::Equals(user.clone()))
        .where_read_at_is_none()
        .where_created_at(DateTimePredicate::After(since))
        .order_by_created_at(SortDirection::Asc)
        .limit(MAX_DIGEST_ITEMS)
        .await?;
    let mut email_on: HashMap<String, bool> = HashMap::new();
    let mut out = Vec::new();
    for n in unread {
        if n.email_sent_at().is_some() {
            continue;
        }
        let kind = n.kind().clone();
        let on = match email_on.get(&kind) {
            Some(on) => *on,
            None => {
                let urgent = crate::kinds::kind(&kind).is_some_and(|k| k.urgent);
                let on = !urgent && effective_channels(valence, user, &kind).await?.email;
                email_on.insert(kind, on);
                on
            }
        };
        if on {
            out.push(n);
        }
    }
    Ok(out)
}

async fn mark_sent(
    valence: &Valence,
    row: NotificationSettings,
    now: DateTime<Utc>,
) -> valence::Result<()> {
    row.get_mutable(valence, valence::use_!(r"When the **notification digest** job sends one person's daily or weekly email, we **update Notification Settings** with when it went out, so the same digest isn't sent twice. Only the digest job writes this field."))
        .set_last_digest_at(now)?
        .commit()
        .await?;
    Ok(())
}

/// Send every digest due at `now`. Returns how many went out.
///
/// Run as System from the host's Chronon job. Without an email channel
/// installed, sends nothing. A person with no verified email, or whose send
/// fails, is skipped and keeps their unsent notifications for the next slot.
///
/// # Errors
///
/// Returns the Valence error when the settings query fails. Per-person
/// failures are logged and skipped.
pub async fn digest_with_valence(valence: &Valence, now: DateTime<Utc>) -> valence::Result<usize> {
    let Some(channel) =
        NotificationChannels::get(valence).and_then(|c| c.channel(ChannelKind::Email).cloned())
    else {
        return Ok(0);
    };
    let mut people: Vec<NotificationSettings> = Vec::new();
    for digest in [
        NotificationSettingsDigest::Daily,
        NotificationSettingsDigest::Weekly,
    ] {
        let name = serde_json::to_value(&digest)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        people.extend(
            NotificationSettings::query(valence, valence::use_!(r"When the **notification digest** job runs each hour, we **list Notification Settings** for people on a daily or weekly digest, to find whose local time is 8 AM. Only the digest job reads them this way."))
                .where_digest(valence::StringPredicate::Equals(name))
                .await?,
        );
    }

    let mut sent = 0;
    for row in people {
        let settings = settings_of(&row);
        let last = row.last_digest_at().copied();
        if !digest_due(&settings, last, now) {
            continue;
        }
        let user = row.user().clone();
        let lookback = if settings.digest == Digest::Weekly {
            Duration::days(7)
        } else {
            Duration::days(1)
        };
        let since = last.unwrap_or(now - lookback);
        let result: valence::Result<bool> = async {
            let rows = digest_rows(valence, &user, since).await?;
            if rows.is_empty() {
                mark_sent(valence, row, now).await?;
                return Ok(false);
            }
            let Some(contact) = verified_contact(&user, valence, ChannelKind::Email).await? else {
                return Ok(false);
            };
            if let Err(err) = channel.deliver(&contact, &digest_message(&rows)).await {
                tracing::warn!(operation = "digest", outcome = "channel_err", error = %err);
                return Ok(false);
            }
            mark_sent(valence, row, now).await?;
            Ok(true)
        }
        .await;
        match result {
            Ok(true) => sent += 1,
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(operation = "digest", outcome = "store_err", error = %err);
            }
        }
    }
    Ok(sent)
}

mod job {
    #![allow(dead_code, missing_docs, clippy::pedantic, clippy::nursery)]

    #[chronon_coordinator_macros::script(
        name = "notifications.digest",
        default_job(job = "notifications-digest", cron = "0 * * * *")
    )]
    pub(crate) async fn notifications_digest(
        ctx: Box<dyn chronon_core::ScriptContext>,
    ) -> anyhow::Result<()> {
        let v = chronon_valence_identity::valence_from_context(&*ctx)?;
        let sent = super::digest_with_valence(&v, chrono::Utc::now()).await?;
        tracing::info!(operation = "digest", sent);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, 0, 0)
            .single()
            .expect("valid")
    }

    fn with(digest: Digest, tz: &str) -> Settings {
        Settings {
            digest,
            time_zone: tz.into(),
            ..Settings::default()
        }
    }

    #[test]
    fn daily_digest_is_due_at_eight_local_once_a_day() {
        let s = with(Digest::Daily, "UTC");
        assert!(digest_due(&s, None, at(2026, 3, 4, 8)));
        assert!(!digest_due(&s, None, at(2026, 3, 4, 9)));
        assert!(!digest_due(&s, Some(at(2026, 3, 4, 8)), at(2026, 3, 4, 8)));
        assert!(digest_due(&s, Some(at(2026, 3, 3, 8)), at(2026, 3, 4, 8)));
    }

    #[test]
    fn weekly_digest_only_on_monday() {
        let s = with(Digest::Weekly, "UTC");
        // 2026-03-02 is a Monday.
        assert!(digest_due(&s, None, at(2026, 3, 2, 8)));
        assert!(!digest_due(&s, None, at(2026, 3, 3, 8)));
    }

    #[test]
    fn digest_uses_local_time_and_never_for_instant() {
        // 8 AM in Tokyo is 23:00 UTC the day before.
        let s = with(Digest::Daily, "Asia/Tokyo");
        assert!(digest_due(&s, None, at(2026, 3, 3, 23)));
        assert!(!digest_due(&s, None, at(2026, 3, 4, 8)));
        assert!(!digest_due(
            &with(Digest::Instant, "UTC"),
            None,
            at(2026, 3, 4, 8)
        ));
    }
}
