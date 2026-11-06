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
//! ## Examples
//!
//! Start with [Send notification](#send-notification). Unit tests in this crate cover relative
//! time, URL sanitize, and DTO mapping; `cargo run -p uf-notifications-core --example system_mint`
//! runs a full in-memory walkthrough. Product hosts that mount `uf-notifications`
//! call `send_notification` then exercise the bell and inbox.

pub mod audience;
pub mod create_policy;
pub mod embedded_surreal;
pub mod events;
pub mod generated;

mod dto;
mod schemas;
mod send;
mod url;

pub use audience::{register_notification_audience, NotificationAudienceCheck};
pub use create_policy::{ScopedNotificationCreate, SCOPED_NOTIFICATION_CREATE};
pub use dto::{notification_to_dto, NotificationDto};
pub use generated::Notification;
pub use send::{send_notification, SendNotification, SendNotificationError};
pub use url::{is_safe_notification_path, sanitize_notification_url};
