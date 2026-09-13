//! Persist + Photon publish for notifications.

use chrono::Utc;
use tracing::Instrument;
use uuid::Uuid;
use valence::Model;

use crate::events::NotificationPushed;
use crate::{notification_to_dto, sanitize_notification_url, Notification, NotificationDto};

/// Cap on free-form notification `kind` strings at send time.
pub const MAX_NOTIFICATION_KIND_CHARS: usize = 64;
/// Cap on notification titles at send time.
pub const MAX_NOTIFICATION_TITLE_CHARS: usize = 256;
/// Cap on notification body text at send time.
pub const MAX_NOTIFICATION_MESSAGE_CHARS: usize = 4_096;
/// Cap on optional `data_json` payload at send time.
pub const MAX_NOTIFICATION_DATA_JSON_CHARS: usize = 8_192;

fn truncate_chars(input: String, max: usize) -> String {
    if input.chars().count() <= max {
        input
    } else {
        input.chars().take(max).collect()
    }
}

/// Parameters for creating a notification via [`send_notification`].
pub struct SendNotification {
    /// The recipient user's Valence record id.
    pub user_id: valence::RecordId,
    /// Free-form notification category (e.g. `"leaderboard"`), used for filtering/icons.
    pub kind: String,
    /// Notification title.
    pub title: String,
    /// Notification body text.
    pub message: String,
    /// Optional link the client should navigate to when the notification is clicked.
    pub url: Option<String>,
    /// Optional structured payload (JSON-encoded) for clients that render richer content.
    pub data_json: Option<String>,
}

/// Failure while constructing or persisting a notification via [`send_notification`].
///
/// Photon publish failures after a successful upsert are logged at `warn` and do
/// **not** surface here — the Valence row is already durable.
#[derive(Debug)]
pub enum SendNotificationError {
    /// [`Notification::new`] rejected the parameters.
    Construct(valence::Error),
    /// Valence upsert failed after the model was built.
    Persist {
        /// UUID string used as the Valence record id.
        notification_id: String,
        /// Underlying Valence error.
        source: valence::Error,
    },
}

impl std::fmt::Display for SendNotificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Construct(err) => write!(f, "construct notification: {err}"),
            Self::Persist {
                notification_id,
                source,
            } => write!(f, "persist notification {notification_id}: {source}"),
        }
    }
}

impl std::error::Error for SendNotificationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Construct(err) => Some(err),
            Self::Persist { source, .. } => Some(source),
        }
    }
}

/// Persist a notification to Valence and publish a real-time event via Photon.
///
/// Records info span `uf_notifications.send` with fields `operation` and
/// `outcome` (`ok`, `persist_err`, or `publish_err`). Span fields omit
/// recipient ids, titles, and message bodies.
///
/// # Errors
///
/// Returns [`SendNotificationError::Construct`] when the Valence model cannot be
/// built, or [`SendNotificationError::Persist`] when upsert fails. A Photon
/// publish failure after a successful upsert is logged and does not fail this
/// call.
///
/// # Examples
///
/// ```rust,no_run
/// # async fn demo() -> Result<(), uf_notifications_core::SendNotificationError> {
/// use uf_notifications_core::{send_notification, SendNotification};
/// use valence::RecordId;
///
/// // Host must provide a Valence handle (system mint or Higgs-scoped).
/// let valence = todo!("Valence from Higgs or system mint");
/// let user_id = RecordId::new("user", "alice");
///
/// let _dto = send_notification(
///     SendNotification {
///         user_id,
///         kind: "leaderboard".into(),
///         title: "Leaderboard update".into(),
///         message: "You moved up to #3.".into(),
///         url: Some("/high-scores".into()),
///         data_json: None,
///     },
///     &valence,
/// )
/// .await?;
/// # Ok(())
/// # }
/// ```
pub async fn send_notification(
    params: SendNotification,
    valence: &valence::Valence,
) -> Result<NotificationDto, SendNotificationError> {
    let notification_uuid = Uuid::new_v4();
    let id = notification_uuid.to_string();
    let user_id_str = params.user_id.to_string();
    let safe_url = sanitize_notification_url(params.url);
    let kind = truncate_chars(params.kind, MAX_NOTIFICATION_KIND_CHARS);
    let title = truncate_chars(params.title, MAX_NOTIFICATION_TITLE_CHARS);
    let message = truncate_chars(params.message, MAX_NOTIFICATION_MESSAGE_CHARS);
    let data_json = params
        .data_json
        .map(|s| truncate_chars(s, MAX_NOTIFICATION_DATA_JSON_CHARS));

    let notification = Notification::new(
        params.user_id,
        kind,
        title,
        message,
        safe_url,
        data_json,
        None,
        Utc::now(),
    )
    .map_err(SendNotificationError::Construct)?;

    let mut dto = notification_to_dto(&notification);
    dto.notification_id = notification_uuid;

    async {
        if let Err(source) = Notification::upsert_used(&id, notification, valence, valence::use_!("upsert Notification in uf-notifications-core/src/send.rs; Valence persistence for this feature path; typed store; visible to session actor / service path.")).await {
            tracing::Span::current().record("outcome", "persist_err");
            tracing::error!(
                operation = "send",
                outcome = "persist_err",
                error_class = "valence_upsert"
            );
            return Err(SendNotificationError::Persist {
                notification_id: id.clone(),
                source,
            });
        }

        match (NotificationPushed {
            user_id: user_id_str.clone(),
            notification_id: id.clone(),
        })
        .publish()
        .await
        {
            Ok(_) => {
                tracing::Span::current().record("outcome", "ok");
            }
            Err(_) => {
                tracing::Span::current().record("outcome", "publish_err");
                tracing::warn!(
                    operation = "send",
                    outcome = "publish_err",
                    error_class = "photon_publish"
                );
            }
        }

        Ok(dto)
    }
    .instrument(tracing::info_span!(
        "uf_notifications.send",
        operation = "send",
        outcome = tracing::field::Empty,
    ))
    .await
}

#[cfg(test)]
mod tests {
    use super::{
        truncate_chars, SendNotificationError, MAX_NOTIFICATION_KIND_CHARS,
        MAX_NOTIFICATION_TITLE_CHARS,
    };

    #[test]
    fn send_notification_error_display_includes_operation() {
        let construct =
            SendNotificationError::Construct(valence::Error::Validation("construct-test".into()));
        assert!(
            construct.to_string().contains("construct notification"),
            "got: {construct}"
        );

        let persist = SendNotificationError::Persist {
            notification_id: "n-1".into(),
            source: valence::Error::database("persist-test"),
        };
        let msg = persist.to_string();
        assert!(msg.contains("persist notification n-1"), "got: {msg}");
        assert!(msg.contains("persist-test"), "got: {msg}");
        assert!(std::error::Error::source(&persist).is_some());
    }

    #[test]
    fn truncate_chars_caps_overlong_fields_sad() {
        let over = "x".repeat(MAX_NOTIFICATION_TITLE_CHARS + 40);
        let capped = truncate_chars(over, MAX_NOTIFICATION_TITLE_CHARS);
        assert_eq!(capped.chars().count(), MAX_NOTIFICATION_TITLE_CHARS);

        let kind = "k".repeat(MAX_NOTIFICATION_KIND_CHARS + 5);
        assert_eq!(
            truncate_chars(kind, MAX_NOTIFICATION_KIND_CHARS)
                .chars()
                .count(),
            MAX_NOTIFICATION_KIND_CHARS
        );
    }
}
