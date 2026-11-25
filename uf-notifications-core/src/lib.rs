//! Persist notifications to Valence and publish live push events over Photon.
//!
//! Backend workers and system jobs call this crate to create durable notification rows
//! and notify connected clients. Auth-scoped list/read server functions live in
//! `uf-notifications-api`; the Orbital bell and inbox UI live in the
//! `uf-notifications` package under unified-field-product.
//!
//! # Features
//!
//! - **Notification send** — Persist a notification row and publish a Photon push so
//!   connected clients refetch their badge. [Get started](#send-notification)
//! - **Client DTO mapping** — Convert a stored [`Notification`] into a wire
//!   [`NotificationDto`] with relative timestamps and sanitized URLs.
//!   [Get started](#client-dto)
//! - **URL sanitization** — Reject open redirects and unsafe paths before a notification
//!   link reaches the client. [Get started](#sanitize-urls)
//! - **Notification kinds and preferences** — Apps register each kind they send;
//!   people turn channels on or off per kind and set digest and quiet hours.
//!   [Get started](#kinds-and-preferences)
//! - **Email and text delivery** — The host installs mail and text channels;
//!   each notification then goes out by email or text when the recipient's
//!   preferences say so, held for quiet hours, batched into daily or weekly
//!   digests, with a retention job and phone verification.
//!   [Get started](#email-and-text-delivery)
//! - **Live push events** — Publish a topic-keyed Photon event after a notification is
//!   created so connected clients can refetch their badge. [Get started](#live-push)
//!
//! # Feature flags
//!
//! | Feature | Default | Purpose |
//! |---------|---------|---------|
//! | `db-sqlite` | yes | SQLite-backed Valence for local dev and tests |
//! | `db-hybrid` | no | Hybrid Surreal/Postgres backend for production hosts |
//! | `hydrate` / `ssr` | no | Workspace feature unification markers (no API surface here) |
//!
//! # Getting started
//!
//! Runnable system mint: `cargo run -p uf-notifications-core --example system_mint`
//!
//! ## Send notification
//!
//! [`send_notification`] is the primary write path for backend code that needs to notify
//! a user. Call it from a worker handler, system job, or other server context after you
//! have a Valence handle scoped to the recipient.
//!
//! Prerequisites: a `valence::Valence` handle (Higgs-scoped for user-owned rows, or
//! `valence::Actor::System` for system mint as in
//! `cargo run -p uf-notifications-core --example system_mint`).
//!
//! ```rust,no_run
//! # async fn demo(valence: &valence::Valence) -> Result<(), uf_notifications_core::SendNotificationError> {
//! use uf_notifications_core::{send_notification, SendNotification};
//! use valence::RecordId;
//!
//! let dto = send_notification(
//!     SendNotification {
//!         user_id: RecordId::new("user", "alice"),
//!         kind: "leaderboard".into(),
//!         title: "Leaderboard update".into(),
//!         message: "You moved up to #3.".into(),
//!         url: Some("/high-scores".into()),
//!         data_json: None,
//!         subject: None,
//!     },
//!     valence,
//! )
//! .await?;
//! assert!(!dto.notification_id.is_nil(), "send returns a fresh notification id");
//! # Ok(())
//! # }
//! ```
//!
//! Set `subject: Some("table:id")` to send a scoped notification as the session actor:
//! the actor must be able to read the subject, and the recipient must pass the
//! audience check the host registered with [`register_notification_audience`]. The
//! row's `sender` is always the acting Valence's actor.
//!
//! Photon publish failures after a successful upsert are logged and do not fail this
//! call — the Valence row is already durable. Next: [live push](#live-push) for the
//! event shape, or `system_mint` for a full in-memory walkthrough.
//!
//! ## Client DTO
//!
//! [`notification_to_dto`] maps a persisted [`Notification`] into the JSON shape
//! clients consume. Relative `created_at` strings are computed at read time; unsafe
//! URLs are stripped the same way as at send time.
//!
//! ```rust,no_run
//! use chrono::Utc;
//! use uf_notifications_core::{notification_to_dto, Notification};
//! use valence::RecordId;
//!
//! let user = RecordId::new("user", "alice");
//! let notification = Notification::new(
//!     user,
//!     "general".into(),
//!     "Title".into(),
//!     "Message".into(),
//!     Some("/inbox".into()),
//!     None,
//!     None,
//!     Utc::now(),
//!     None,
//!     None,
//!     None,
//!     None,
//!     None,
//! )
//! .expect("valid notification");
//!
//! let dto = notification_to_dto(&notification);
//! assert_eq!(dto.title, "Title");
//! assert_eq!(dto.url.as_deref(), Some("/inbox"));
//! assert!(!dto.is_read);
//! ```
//!
//! API crates re-export this helper under `ssr`; UI crates receive the DTO from server
//! functions. Next: [`NotificationDto`] field reference.
//!
//! ## Sanitize URLs
//!
//! [`sanitize_notification_url`] keeps notification action links on same-origin relative
//! paths. Protocol-relative URLs, `/auth` and `/api` endpoints, and smuggled absolute
//! URLs are rejected so clients fall back to the inbox.
//!
//! ```rust
//! use uf_notifications_core::{is_safe_notification_path, sanitize_notification_url};
//!
//! assert_eq!(
//!     sanitize_notification_url(Some("/high-scores".into())).as_deref(),
//!     Some("/high-scores"),
//! );
//! assert_eq!(sanitize_notification_url(Some("//evil.example".into())), None);
//! assert!(!is_safe_notification_path("/auth/signin"));
//! ```
//!
//! [`send_notification`] and [`notification_to_dto`] call this helper automatically.
//! Next: unit tests in `url.rs` for backslash and control-character sad paths.
//!
//! ## Live push
//!
//! [`events::NotificationPushed`] publishes on Photon topic `user.notifications`, keyed by
//! `user_id`. [`send_notification`] emits this event after a successful upsert; hosts
//! with a Photon runtime can also publish directly when bridging external feeds.
//!
//! Prerequisites: install a Photon runtime for the process (same wiring as
//! `system_mint` / product hosts) before calling [`.publish()`](events::NotificationPushed::publish).
//! Without a runtime, publish returns `Err`. Clients refetch badges via
//! `uf-notifications-api` `subscribe_get_unread_count` (`ssr` or `hydrate`) over
//! `/ws/notifications`.
//!
//! ```rust,no_run
//! # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! use uf_notifications_core::events::NotificationPushed;
//!
//! let published = (NotificationPushed {
//!     user_id: "user:alice".into(),
//!     notification_id: "550e8400-e29b-41d4-a716-446655440000".into(),
//! })
//! .publish()
//! .await;
//! assert!(
//!     published.is_ok(),
//!     "Photon runtime missing or publish failed: {published:?}"
//! );
//! # Ok(())
//! # }
//! ```
//!
//! Next: [`events`] module for the topic macro contract, or
//! `uf-notifications-api` live unread badge for the subscribe/refetch path.
//!
//! ## Kinds and preferences
//!
//! Register each kind next to the app's `uf_app!` with
//! `inventory::submit! { NotificationKind { .. } }` (see [`mod@kinds`]) and pass its id as
//! [`SendNotification::kind`]. [`effective_channels`] answers which channels a kind uses
//! for one person: their saved choice from [`set_channel`], else the kind's defaults,
//! with email and text off until the host's [`ContactVerification`] says the contact is
//! verified. [`load_settings`] and [`save_settings`] hold digest and quiet hours. All of
//! these run as the person through Valence privacy, and both tables are deleted with
//! the account.
//!
//! ```rust,no_run
//! # async fn demo(valence: &valence::Valence) -> valence::Result<()> {
//! use uf_notifications_core::effective_channels;
//! use valence::RecordId;
//!
//! let alice = RecordId::new("user", "alice");
//! let channels = effective_channels(valence, &alice, "magnon.assigned").await?;
//! assert!(channels.in_app);
//! # Ok(())
//! # }
//! ```
//!
//! ## Email and text delivery
//!
//! The in-app row is always written. For email and text the host wires three
//! things at boot; a host that wires none sends in-app only, as before:
//!
//! 1. Channels: implement [`NotificationChannel`] over its mail and SMS
//!    providers and install them with [`NotificationChannels::install`] on the
//!    Valence builder. [`channel_configured`] tells the preferences page
//!    whether to show Text.
//! 2. A Boson runtime (the `notifications.deliver` task) and a Chronon backend
//!    passed to [`register_delivery_scheduler`] for messages held by quiet
//!    hours.
//! 3. Chronon default jobs `notifications-digest` (hourly) and
//!    `notifications-retention` (daily), registered with a System Valence.
//!
//! The host's [`ContactVerification`] also returns the verified email address
//! through `verified_email`, answering for the delivery actor
//! ([`delivery_actor`]) and System. People verify a phone with
//! [`start_phone_verification`] and [`confirm_phone_verification`]. See
//! [`mod@delivery`] for routing, [`mod@digest`] and [`mod@retention`] for the
//! jobs.
//!
//! ```rust,no_run
//! # fn demo(mail: std::sync::Arc<dyn uf_notifications_core::NotificationChannel>) {
//! use uf_notifications_core::NotificationChannels;
//!
//! let builder = NotificationChannels::install(
//!     valence::Valence::builder(),
//!     NotificationChannels::new().with(mail),
//! );
//! # let _ = builder;
//! # }
//! ```
//!
//! ## Examples
//!
//! Start with [Send notification](#send-notification). Unit tests in this crate cover relative
//! time, URL sanitize, and DTO mapping; `cargo run -p uf-notifications-core --example system_mint`
//! runs a full in-memory walkthrough. Product hosts that mount `uf-notifications`
//! call `send_notification` then exercise the bell and inbox.

pub mod audience;
pub mod channels;
pub mod create_policy;
pub mod delivery;
pub mod delivery_policy;
pub mod digest;
pub mod embedded_surreal;
pub mod events;
pub mod generated;
pub mod kinds;
pub mod phone;
pub mod retention;

mod dto;
mod schemas;
mod send;
mod url;

pub use audience::{register_notification_audience, NotificationAudienceCheck};
pub use channels::{channel_configured, ChannelKind, NotificationChannel, NotificationChannels};
pub use create_policy::{ScopedNotificationCreate, SCOPED_NOTIFICATION_CREATE};
pub use delivery::register_delivery_scheduler;
pub use delivery_policy::{delivery_actor, NOTIFICATION_DELIVERY};
pub use dto::{notification_to_dto, NotificationDto};
pub use generated::{
    Notification, NotificationContact, NotificationPreference, NotificationSettings,
};
/// Re-exported so senders can register kinds with
/// `uf_notifications_core::inventory::submit!`.
pub use inventory;
pub use kinds::{
    contact_verified, effective_channels, kind, kinds, kinds_for_app, load_preferences,
    load_settings, mask_unverified, register_contact_verification, save_settings, set_channel,
    Channel, Channels, ContactVerification, Digest, NotificationKind, PreferenceError, Settings,
    MAX_TIME_ZONE_CHARS,
};
pub use phone::{confirm_phone_verification, start_phone_verification, PhoneVerificationError};
pub use send::{send_notification, SendNotification, SendNotificationError};
pub use url::{is_safe_notification_path, sanitize_notification_url};
