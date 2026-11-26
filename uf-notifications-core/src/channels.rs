//! Email and text delivery port, provided by the host.
//!
//! This crate never talks to a mail or SMS provider. The host implements
//! [`NotificationChannel`] once per channel it supports and installs them on
//! its Valence builder with [`NotificationChannels::install`]. A host without
//! an email channel never sends email, and one without a text channel never
//! sends text; with neither, [`send_notification`](crate::send_notification)
//! behaves exactly as an in-app-only send.
//!
//! ```rust
//! use std::sync::Arc;
//! use uf_notifications_core::channels::{
//!     ChannelError, ChannelKind, Contact, NotificationChannel, NotificationChannels,
//!     OutgoingNotification,
//! };
//!
//! struct LogMail;
//!
//! #[async_trait::async_trait]
//! impl NotificationChannel for LogMail {
//!     fn kind(&self) -> ChannelKind {
//!         ChannelKind::Email
//!     }
//!     async fn deliver(&self, to: &Contact, msg: &OutgoingNotification) -> Result<(), ChannelError> {
//!         tracing::info!(to = to.address(), subject = %msg.title, "would send");
//!         Ok(())
//!     }
//! }
//!
//! let builder = NotificationChannels::install(
//!     valence::Valence::builder(),
//!     NotificationChannels::new().with(Arc::new(LogMail)),
//! );
//! # let _ = builder;
//! ```

use std::fmt;
use std::sync::Arc;

/// A delivery channel other than in-app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChannelKind {
    /// Email to a verified address.
    Email,
    /// Text message to a verified phone number.
    Text,
}

impl fmt::Display for ChannelKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Email => "email",
            Self::Text => "text",
        })
    }
}

/// Where a channel delivers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Contact {
    /// A verified email address.
    Email(String),
    /// A phone number in E.164 form, for example `+353861234567`.
    Phone(String),
}

impl Contact {
    /// The address or number.
    pub fn address(&self) -> &str {
        match self {
            Self::Email(a) | Self::Phone(a) => a,
        }
    }
}

/// One message a channel sends: a single notification, a digest, or a
/// phone verification code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingNotification {
    /// Subject line for email; the first line for text.
    pub title: String,
    /// Body text.
    pub message: String,
    /// Same-origin path to open, already sanitized.
    pub url: Option<String>,
    /// Kind id, or `notifications.digest` / `notifications.phone_code`.
    pub kind_id: String,
    /// The notification's id; `None` for a digest or a verification code.
    pub notification_id: Option<String>,
}

/// Kind id on an [`OutgoingNotification`] that carries a digest.
pub const DIGEST_KIND_ID: &str = "notifications.digest";
/// Kind id on an [`OutgoingNotification`] that carries a phone code.
pub const PHONE_CODE_KIND_ID: &str = "notifications.phone_code";

/// A channel could not deliver. The text is recorded on the notification's
/// `delivery_error`, so keep it free of addresses and message bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelError(pub String);

impl fmt::Display for ChannelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ChannelError {}

/// Sends one kind of message through a provider the host owns.
#[async_trait::async_trait]
pub trait NotificationChannel: Send + Sync {
    /// Which channel this is.
    fn kind(&self) -> ChannelKind;

    /// Send `msg` to `to`.
    ///
    /// # Errors
    ///
    /// [`ChannelError`] when the provider refuses or fails. Delivery records
    /// it and retries through Boson.
    async fn deliver(&self, to: &Contact, msg: &OutgoingNotification) -> Result<(), ChannelError>;
}

/// The channels a host installed, read back from Valence with
/// [`NotificationChannels::get`].
#[derive(Clone, Default)]
pub struct NotificationChannels {
    email: Option<Arc<dyn NotificationChannel>>,
    text: Option<Arc<dyn NotificationChannel>>,
}

impl fmt::Debug for NotificationChannels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NotificationChannels")
            .field("email", &self.email.is_some())
            .field("text", &self.text.is_some())
            .finish()
    }
}

impl NotificationChannels {
    /// No channels.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add `channel` under its [`NotificationChannel::kind`], replacing any
    /// earlier channel of that kind.
    #[must_use]
    pub fn with(mut self, channel: Arc<dyn NotificationChannel>) -> Self {
        match channel.kind() {
            ChannelKind::Email => self.email = Some(channel),
            ChannelKind::Text => self.text = Some(channel),
        }
        self
    }

    /// Install `channels` as a Valence extension on `builder`. Every handle
    /// built from it, including `with_actor` clones, sees them.
    pub fn install(builder: valence::ValenceBuilder, channels: Self) -> valence::ValenceBuilder {
        builder.extension(Arc::new(channels))
    }

    /// The channels installed on `valence`, if any.
    pub fn get(valence: &valence::Valence) -> Option<Arc<Self>> {
        valence.extension::<Self>()
    }

    /// The channel of `kind`, if installed.
    pub fn channel(&self, kind: ChannelKind) -> Option<&Arc<dyn NotificationChannel>> {
        match kind {
            ChannelKind::Email => self.email.as_ref(),
            ChannelKind::Text => self.text.as_ref(),
        }
    }

    /// Whether any channel is installed.
    pub fn is_empty(&self) -> bool {
        self.email.is_none() && self.text.is_none()
    }
}

/// Whether the host installed a channel of `kind` on `valence`. The
/// preferences page hides Text when this is `false`.
pub fn channel_configured(valence: &valence::Valence, kind: ChannelKind) -> bool {
    NotificationChannels::get(valence).is_some_and(|c| c.channel(kind).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(ChannelKind);

    #[async_trait::async_trait]
    impl NotificationChannel for Fake {
        fn kind(&self) -> ChannelKind {
            self.0
        }
        async fn deliver(&self, _: &Contact, _: &OutgoingNotification) -> Result<(), ChannelError> {
            Ok(())
        }
    }

    #[test]
    fn with_files_channels_by_kind() {
        let none = NotificationChannels::new();
        assert!(none.is_empty());
        let email_only = NotificationChannels::new().with(Arc::new(Fake(ChannelKind::Email)));
        assert!(email_only.channel(ChannelKind::Email).is_some());
        assert!(email_only.channel(ChannelKind::Text).is_none());
        assert!(!email_only.is_empty());
    }

    #[test]
    fn contact_address_returns_either_form() {
        assert_eq!(Contact::Email("a@b.c".into()).address(), "a@b.c");
        assert_eq!(
            Contact::Phone("+15551234567".into()).address(),
            "+15551234567"
        );
        assert_eq!(ChannelKind::Text.to_string(), "text");
    }
}
