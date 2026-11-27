//! Wire DTOs. The notification DTO has a client/hydrate copy here (SSR
//! re-exports core's); the kind, preference and filter DTOs are shared.

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
    /// The stored kind, for example `magnon.assigned`.
    pub kind_id: Option<String>,
    /// The kind's label; `None` for an unregistered kind.
    pub kind_label: Option<String>,
    /// The sending app's id; `None` for an unregistered kind.
    pub app_id: Option<String>,
    /// Whether the kind asks the reader to act.
    pub asks_action: bool,
}

/// Which channels a kind reaches a person on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChannelsDto {
    /// The bell and inbox.
    pub in_app: bool,
    /// Email.
    pub email: bool,
    /// Text message.
    pub text: bool,
}

/// One delivery channel, for [`crate::set_notification_channel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NotificationChannel {
    /// The bell and inbox.
    InApp,
    /// Email.
    Email,
    /// Text message.
    Text,
}

/// A registered notification kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindDto {
    /// `<app>.<name>`, for example `magnon.assigned`.
    pub id: String,
    /// The sending app's id.
    pub app: String,
    /// Short label.
    pub label: String,
    /// One sentence on when this kind is sent.
    pub description: String,
    /// Channels used until the person changes them.
    pub defaults: ChannelsDto,
    /// The notification asks the reader to act.
    pub asks_action: bool,
    /// Skips a digest and goes out right away.
    pub urgent: bool,
}

/// One kind with the channels it uses for the signed-in person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindPreferenceDto {
    /// The kind.
    pub kind: KindDto,
    /// Effective channels: the saved choice, else the defaults, with email
    /// and text off until the contact is verified.
    pub channels: ChannelsDto,
}

/// How often email goes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DigestFrequency {
    /// One email per notification.
    #[default]
    Instant,
    /// One email a day.
    Daily,
    /// One email a week.
    Weekly,
}

/// Digest and quiet-hours settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsDto {
    /// How often email goes out.
    pub digest: DigestFrequency,
    /// Whether quiet hours hold email and text.
    pub quiet_enabled: bool,
    /// Start of quiet hours, local `"HH:MM"`.
    pub quiet_start: String,
    /// End of quiet hours, local `"HH:MM"`.
    pub quiet_end: String,
    /// IANA time zone, for example `"Europe/Dublin"`.
    pub time_zone: String,
}

/// Everything the preferences page shows for the signed-in person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreferencesDto {
    /// Every registered kind, sorted by app then label, with effective channels.
    pub kinds: Vec<KindPreferenceDto>,
    /// Digest and quiet hours.
    pub settings: SettingsDto,
    /// Whether the person's email address is verified.
    pub email_verified: bool,
    /// Whether the person has a verified phone for text messages.
    pub text_verified: bool,
}

/// Inbox filters on top of the read filter and search.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct NotificationInboxFilter {
    /// Only kinds that ask the reader to act.
    pub asks_action: bool,
    /// Only kinds registered by this app.
    pub app_id: Option<String>,
}

impl NotificationInboxFilter {
    /// Whether the filter narrows anything.
    pub fn is_active(&self) -> bool {
        self.asks_action || self.app_id.is_some()
    }
}

#[cfg(feature = "ssr")]
mod convert {
    use super::{ChannelsDto, DigestFrequency, KindDto, NotificationChannel, SettingsDto};
    use uf_notifications_core::{Channel, Channels, Digest, NotificationKind, Settings};

    impl From<Channels> for ChannelsDto {
        fn from(c: Channels) -> Self {
            Self {
                in_app: c.in_app,
                email: c.email,
                text: c.text,
            }
        }
    }

    impl From<NotificationChannel> for Channel {
        fn from(c: NotificationChannel) -> Self {
            match c {
                NotificationChannel::InApp => Self::InApp,
                NotificationChannel::Email => Self::Email,
                NotificationChannel::Text => Self::Text,
            }
        }
    }

    impl From<&NotificationKind> for KindDto {
        fn from(k: &NotificationKind) -> Self {
            Self {
                id: k.id.to_string(),
                app: k.app.to_string(),
                label: k.label.to_string(),
                description: k.description.to_string(),
                defaults: k.defaults.into(),
                asks_action: k.asks_action,
                urgent: k.urgent,
            }
        }
    }

    impl From<Settings> for SettingsDto {
        fn from(s: Settings) -> Self {
            Self {
                digest: match s.digest {
                    Digest::Instant => DigestFrequency::Instant,
                    Digest::Daily => DigestFrequency::Daily,
                    Digest::Weekly => DigestFrequency::Weekly,
                },
                quiet_enabled: s.quiet_enabled,
                quiet_start: s.quiet_start,
                quiet_end: s.quiet_end,
                time_zone: s.time_zone,
            }
        }
    }

    impl From<SettingsDto> for Settings {
        fn from(s: SettingsDto) -> Self {
            Self {
                digest: match s.digest {
                    DigestFrequency::Instant => Digest::Instant,
                    DigestFrequency::Daily => Digest::Daily,
                    DigestFrequency::Weekly => Digest::Weekly,
                },
                quiet_enabled: s.quiet_enabled,
                quiet_start: s.quiet_start,
                quiet_end: s.quiet_end,
                time_zone: s.time_zone,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_dto_serde_round_trip() {
        let dto = PreferencesDto {
            kinds: vec![KindPreferenceDto {
                kind: KindDto {
                    id: "magnon.assigned".into(),
                    app: "magnon".into(),
                    label: "Assigned to you".into(),
                    description: "Someone assigned you a task.".into(),
                    defaults: ChannelsDto {
                        in_app: true,
                        email: true,
                        text: false,
                    },
                    asks_action: true,
                    urgent: false,
                },
                channels: ChannelsDto {
                    in_app: true,
                    email: false,
                    text: false,
                },
            }],
            settings: SettingsDto {
                digest: DigestFrequency::Weekly,
                quiet_enabled: true,
                quiet_start: "22:00".into(),
                quiet_end: "07:00".into(),
                time_zone: "UTC".into(),
            },
            email_verified: false,
            text_verified: false,
        };
        let json = serde_json::to_string(&dto).expect("serialize");
        let back: PreferencesDto = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, dto);
    }

    #[test]
    fn inbox_filter_default_is_inactive() {
        assert!(!NotificationInboxFilter::default().is_active());
        let by_app = NotificationInboxFilter {
            app_id: Some("magnon".into()),
            ..Default::default()
        };
        assert!(by_app.is_active());
        let json = serde_json::to_string(&by_app).expect("serialize");
        assert_eq!(
            serde_json::from_str::<NotificationInboxFilter>(&json).expect("deserialize"),
            by_app
        );
    }
}

#[cfg(all(test, not(feature = "ssr")))]
mod created_at_rfc3339_tests {
    use super::NotificationDto;

    #[test]
    fn dto_without_created_at_rfc3339_deserializes() {
        let json = r#"{
            "notification_id": "00000000-0000-0000-0000-000000000000",
            "title": "Title",
            "message": "Message",
            "created_at": "5m ago",
            "url": null,
            "is_read": false,
            "kind_id": null,
            "kind_label": null,
            "app_id": null,
            "asks_action": false
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
