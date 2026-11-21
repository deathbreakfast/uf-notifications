//! Client/hydrate DTO when SSR is not enabled (SSR re-exports core).

#[cfg(not(feature = "ssr"))]
use serde::{Deserialize, Serialize};

/// Client/hydrate copy of the notification DTO (matches core layout; SSR uses `uf-notifications-core`).
#[cfg(not(feature = "ssr"))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationDto {
    /// Notification's unique id.
    pub notification_id: uuid::Uuid,
    /// Notification title.
    pub title: String,
    /// Notification body text.
    pub message: String,
    /// Human-friendly relative time (e.g. `"5m ago"`). New UI should render
    /// [`Self::created_at_rfc3339`] instead.
    pub created_at: String,
    /// When the notification was created, as an RFC 3339 UTC string (for Orbital's
    /// `Timestamp`). Empty when the payload came from a server that predates the field.
    #[serde(default)]
    pub created_at_rfc3339: String,
    /// Optional link the client should navigate to when the notification is clicked.
    pub url: Option<String>,
    /// Whether the recipient has marked this notification as read.
    pub is_read: bool,
}

#[cfg(all(test, not(feature = "ssr")))]
mod tests {
    use super::NotificationDto;

    #[test]
    fn dto_without_created_at_rfc3339_deserializes() {
        let json = r#"{
            "notification_id": "00000000-0000-0000-0000-000000000000",
            "title": "Title",
            "message": "Message",
            "created_at": "5m ago",
            "url": null,
            "is_read": false
        }"#;
        let dto: NotificationDto = serde_json::from_str(json).expect("older payload");
        assert_eq!(dto.created_at_rfc3339, "");

        let dto = NotificationDto {
            created_at_rfc3339: "2026-11-20T10:00:00+00:00".into(),
            ..dto
        };
        let back: NotificationDto =
            serde_json::from_str(&serde_json::to_string(&dto).expect("serialize"))
                .expect("round trip");
        assert_eq!(back, dto);
    }
}
