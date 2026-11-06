//! Client wire shape and relative-time formatting.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{sanitize_notification_url, Notification};

/// DTO for notification API responses (serialized to client).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationDto {
    /// Notification's unique id.
    pub notification_id: Uuid,
    /// Notification title.
    pub title: String,
    /// Notification body text.
    pub message: String,
    /// Human-friendly relative time (e.g. `"5m ago"`), computed at read time by
    /// [`notification_to_dto`] rather than stored.
    pub created_at: String,
    /// Optional link the client should navigate to when the notification is clicked.
    pub url: Option<String>,
    /// Whether the recipient has marked this notification as read.
    pub is_read: bool,
}

pub(crate) fn format_relative_time(dt: &DateTime<Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(*dt);

    if duration.num_minutes() < 1 {
        return "just now".to_string();
    }
    if duration.num_minutes() < 60 {
        return format!("{}m ago", duration.num_minutes());
    }
    if duration.num_hours() < 24 {
        return format!("{}h ago", duration.num_hours());
    }
    format!("{}d ago", duration.num_days())
}

/// Convert a persisted [`Notification`] into the client-facing [`NotificationDto`], computing
/// a human-friendly relative timestamp from `created_at`.
///
/// # Examples
///
/// ```rust,no_run
/// use chrono::Utc;
/// use uf_notifications_core::{notification_to_dto, Notification};
/// use valence::RecordId;
///
/// let user = RecordId::new("user", "alice");
/// let notification = Notification::new(
///     user,
///     "general".into(),
///     "Title".into(),
///     "Message".into(),
///     Some("/inbox".into()),
///     None,
///     None,
///     Utc::now(),
///     None,
///     None,
/// )
/// .expect("valid notification");
///
/// let dto = notification_to_dto(&notification);
/// assert_eq!(dto.title, "Title");
/// assert_eq!(dto.url.as_deref(), Some("/inbox"));
/// assert!(!dto.is_read);
/// ```
pub fn notification_to_dto(n: &Notification) -> NotificationDto {
    let notification_id = n
        .id()
        .and_then(|thing| {
            let s = thing.to_string();
            s.split(':').next_back().and_then(|id| {
                let trimmed = id.trim_matches(|c| c == '⟨' || c == '⟩');
                Uuid::parse_str(trimmed).ok()
            })
        })
        .unwrap_or_default();

    NotificationDto {
        notification_id,
        title: n.title().clone(),
        message: n.message().clone(),
        created_at: format_relative_time(n.created_at()),
        url: sanitize_notification_url(n.url().cloned()),
        is_read: n.read_at().is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn format_relative_time_buckets() {
        let now = Utc::now();
        assert_eq!(format_relative_time(&now), "just now");
        assert_eq!(
            format_relative_time(&(now - Duration::minutes(5))),
            "5m ago"
        );
        assert_eq!(format_relative_time(&(now - Duration::hours(3))), "3h ago");
        assert_eq!(format_relative_time(&(now - Duration::days(2))), "2d ago");
    }

    #[test]
    fn notification_to_dto_maps_fields_and_read_flag() {
        let user = valence::RecordId::new("user", "alice");
        let created = Utc::now() - Duration::minutes(12);
        let notification = Notification::new(
            user,
            "leaderboard".into(),
            "Title".into(),
            "Message".into(),
            Some("/high-scores".into()),
            None,
            None,
            created,
            None,
            None,
        )
        .expect("Notification::new");

        let dto = notification_to_dto(&notification);
        assert_eq!(dto.title, "Title");
        assert_eq!(dto.message, "Message");
        assert_eq!(dto.url.as_deref(), Some("/high-scores"));
        assert!(!dto.is_read);
        assert_eq!(dto.created_at, "12m ago");
    }

    #[test]
    fn notification_to_dto_strips_unsafe_url() {
        let user = valence::RecordId::new("user", "alice");
        let notification = Notification::new(
            user,
            "general".into(),
            "Title".into(),
            "Message".into(),
            Some("//evil.example/phish".into()),
            None,
            None,
            Utc::now(),
            None,
            None,
        )
        .expect("Notification::new");

        let dto = notification_to_dto(&notification);
        assert_eq!(dto.url, None);
    }

    #[test]
    fn notification_to_dto_strips_backslash_and_url_smuggle_sad() {
        let user = valence::RecordId::new("user", "alice");
        for bad in [
            "/\\evil.example",
            "/https://evil.example",
            "/\tevil.example",
        ] {
            let notification = Notification::new(
                user.clone(),
                "general".into(),
                "Title".into(),
                "Message".into(),
                Some(bad.into()),
                None,
                None,
                Utc::now(),
                None,
                None,
            )
            .expect("Notification::new");
            let dto = notification_to_dto(&notification);
            assert_eq!(dto.url, None, "expected strip for `{bad}`");
        }
    }

    #[test]
    fn notification_to_dto_marks_read_when_read_at_set_happy_path() {
        let user = valence::RecordId::new("user", "alice");
        let created = Utc::now() - Duration::minutes(3);
        let read_at = Utc::now() - Duration::minutes(1);
        let notification = Notification::new(
            user,
            "general".into(),
            "Title".into(),
            "Message".into(),
            Some("/inbox".into()),
            None,
            Some(read_at),
            created,
            None,
            None,
        )
        .expect("Notification::new");

        let dto = notification_to_dto(&notification);
        assert!(dto.is_read);
        assert_eq!(dto.created_at, "3m ago");
        assert_eq!(dto.url.as_deref(), Some("/inbox"));
    }
}
