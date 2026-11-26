//! Quiet hours and channel routing: pure functions with no I/O.
//!
//! [`route`] decides, for one notification, whether email and text go out
//! now, wait for quiet hours to end, go to the digest, or stay off.
//! [`deliver_with_valence`](crate::delivery::deliver_with_valence) runs the
//! result.

use std::str::FromStr;

use chrono::{DateTime, Duration, NaiveTime, TimeZone, Utc};

use crate::kinds::{Channels, Digest, Settings};

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
}
