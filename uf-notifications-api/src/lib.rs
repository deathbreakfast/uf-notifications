//! Auth-scoped notification server functions backed by Valence and Photon.
//!
//! Leptos `#[server]` read and write paths for the signed-in user. Persistence
//! and live push publish live in [`send_notification`] (`uf-notifications-core`);
//! the Orbital bell and inbox UI live in unified-field-product's `uf-notifications`.
//!
//! # Features
//!
//! - **Notification list** — Page, search, and list the current user's notifications
//!   from a client component or inbox view. [Get started](#list-notifications)
//! - **Unread preview** — Load the first unread rows for the bell dropdown and
//!   infinite-scroll paging. [Get started](#unread-preview)
//! - **Synced unread count** — Photon-backed unread badge count that refetches after
//!   each push. [Get started](#live-unread-badge)
//! - **Mark read** — Mark one notification or every unread row as read for the
//!   session user. [Get started](#mark-read)
//! - **Session user id** — Parse Higgs `table:id` session values into table and id
//!   parts for SSR auth helpers. [Get started](#session-user-id)
//! - **Dev test mint** — Create a notification for the signed-in user in development
//!   builds (`dev-tools` feature). [Get started](#dev-test-notification)
//!
//! # Getting started
//!
//! From a signed-in Leptos client (host must enable `ssr` on this crate):
//!
//! ```rust,ignore
//! use uf_notifications_api::{get_unread_count, list_notifications};
//!
//! let count = get_unread_count().await?;
//! let recent = list_notifications().await?;
//! ```
//!
//! ## List notifications
//!
//! [`list_notifications`] returns up to 50 recent rows for the authenticated session
//! user, newest first. Use it for a simple inbox feed; prefer [`get_notifications_page`]
//! when you need search, read filters, or offset paging.
//!
//! Prerequisites: signed-in session (Higgs request context on the server) and `ssr`
//! enabled on `uf-notifications-api`. Seed at least one row (for example via
//! [dev test notification](#dev-test-notification) with `dev-tools`) before asserting
//! list contents.
//!
//! ```rust,ignore
//! # async fn demo() -> Result<(), leptos::prelude::ServerFnError> {
//! use uf_notifications_api::{create_test_notification, list_notifications};
//!
//! // `dev-tools` feature required for create_test_notification.
//! create_test_notification(
//!     "Leaderboard update".into(),
//!     "You moved up to #3.".into(),
//!     Some("/high-scores".into()),
//! )
//! .await?;
//! let recent = list_notifications().await?;
//! assert!(
//!     recent.iter().any(|n| n.title == "Leaderboard update"),
//!     "minted title appears in the signed-in list"
//! );
//! assert!(recent.len() <= 50, "list caps at fifty rows");
//! # Ok(())
//! # }
//! ```
//!
//! Auth failures return `"You must be signed in"`; Valence errors map to
//! `"notification store failed"` without engine text. Next: [unread preview](#unread-preview)
//! for bell dropdown rows, or [`get_notifications_page`] for search and filters.
//!
//! ## Unread preview
//!
//! [`get_unread_notifications_preview`] offers the ten newest unread notifications
//! for the bell dropdown. [`get_unread_notifications_page`] supports offset/limit paging
//! for infinite scroll in the same dropdown.
//!
//! Prerequisites: same signed-in `ssr` context as [list notifications](#list-notifications).
//!
//! ```rust,ignore
//! # async fn demo() -> Result<(), leptos::prelude::ServerFnError> {
//! use uf_notifications_api::{create_test_notification, get_unread_notifications_preview};
//!
//! create_test_notification(
//!     "Unread preview row".into(),
//!     "Bell dropdown seed".into(),
//!     Some("/notifications".into()),
//! )
//! .await?;
//! let preview = get_unread_notifications_preview().await?;
//! assert!(
//!     preview.iter().any(|n| n.title == "Unread preview row" && !n.is_read),
//!     "minted unread row appears in preview"
//! );
//! assert!(preview.len() <= 10, "preview returns at most ten unread rows");
//! # Ok(())
//! # }
//! ```
//!
//! `offset` and `limit` on the page variant are clamped server-side; invalid values
//! do not produce a separate error. Next: [live unread badge](#live-unread-badge) for
//! the synced count resource.
//!
//! ## Live unread badge
//!
//! [`get_unread_count`] exposes the unread total for the bell badge. When `ssr` or
//! `hydrate` is enabled, Photon subscribes on topic `user.notifications` and emits
//! [`subscribe_get_unread_count`] so the host refetches after each push. Mount the
//! Photon WebSocket route at `/ws/notifications` during host startup alongside Higgs
//! session context.
//!
//! Prerequisites: `ssr` or `hydrate` on this crate, signed-in user, and `/ws/notifications`
//! mounted on the Axum router at boot.
//!
//! ```rust,ignore
//! # async fn demo() -> Result<(), leptos::prelude::ServerFnError> {
//! use uf_notifications_api::{
//!     create_test_notification, get_unread_count, subscribe_get_unread_count,
//!     MAX_NOTIFICATION_COUNT_CAP,
//! };
//! use leptos::prelude::*;
//!
//! let before = get_unread_count().await?;
//! let bumps = RwSignal::new(0u32);
//! let trigger = subscribe_get_unread_count(move || {
//!     bumps.update(|n| *n = n.wrapping_add(1));
//! });
//! // Bell pattern: Resource keyed on `trigger` refetches after each Photon bump.
//! let count_res = Resource::new(move || trigger.get(), |_| get_unread_count());
//! create_test_notification(
//!     "Badge bump".into(),
//!     "Unread count must rise for the signed-in session".into(),
//!     None,
//! )
//! .await?;
//! let after = get_unread_count().await?;
//! assert!(
//!     after > before,
//!     "mint increases unread count for the signed-in session"
//! );
//! assert!(after <= MAX_NOTIFICATION_COUNT_CAP as usize);
//! // Until `/ws/notifications` delivers a push, trigger/bumps stay at 0.
//! assert_eq!(trigger.get_untracked(), 0);
//! assert_eq!(bumps.get_untracked(), 0);
//! let _keep = (trigger, count_res, bumps);
//! # Ok(())
//! # }
//! ```
//!
//! Builds with neither `ssr` nor `hydrate` compile a stub [`subscribe_get_unread_count`]
//! whose trigger stays at `0`. Next: `uf-notifications` bell wiring, or
//! [`uf_notifications_core::events::NotificationPushed`] for the publish side.
//!
//! ## Mark read
//!
//! [`mark_notification_read`] sets `read_at` on one row; [`mark_all_notifications_read`]
//! walks every unread notification for the session user and returns how many rows
//! committed. Missing or privacy-hidden ids complete with `Ok(())` so callers cannot
//! enumerate other users' notifications.
//!
//! Prerequisites: signed-in `ssr` session and a notification id from list or preview.
//!
//! ```rust,ignore
//! # async fn demo() -> Result<(), leptos::prelude::ServerFnError> {
//! use uf_notifications_api::{
//!     create_test_notification, get_unread_count, get_unread_notifications_preview,
//!     mark_all_notifications_read, mark_notification_read,
//! };
//!
//! create_test_notification("Mark-me".into(), "body".into(), None).await?;
//! let preview = get_unread_notifications_preview().await?;
//! let id = preview
//!     .iter()
//!     .find(|n| n.title == "Mark-me")
//!     .map(|n| n.notification_id)
//!     .expect("minted unread row");
//! let before = get_unread_count().await?;
//! mark_notification_read(id).await?;
//! let after = get_unread_count().await?;
//! assert!(after < before || before == 0, "mark-read lowers unread when a row existed");
//! let updated = mark_all_notifications_read().await?;
//! assert!(updated <= 500, "mark-all returns committed row count");
//! # Ok(())
//! # }
//! ```
//!
//! Individual persist failures during mark-all are skipped; the returned count may
//! be smaller than the unread query. Next: [`mark_notification_unread`] to reverse a
//! single row.
//!
//! ## Session user id
//!
//! [`parse_session_user_id`] splits Higgs session values (`table:id`) into table and
//! id parts. SSR auth helpers call this before Valence queries; the function is
//! feature-free so unit tests cover parse contracts without a request context.
//!
//! ```rust
//! use uf_notifications_api::parse_session_user_id;
//!
//! let (table, id) = parse_session_user_id("user:alice").expect("valid");
//! assert_eq!(table, "user");
//! assert_eq!(id, "alice");
//! ```
//!
//! Malformed values (missing `:`, empty table or id) return [`SessionUserIdError`]
//! with the rejected input preserved. Next: [list notifications](#list-notifications)
//! (SSR paths parse the session the same way before querying Valence).
//!
//! ## Dev test notification
//!
//! `create_test_notification` mints a row for the authenticated session user through
//! the same [`send_notification`] persist and Photon publish path as backend callers.
//! Enable the `dev-tools` feature on `uf-notifications-api` (and `uf-notifications` if
//! you re-export from the UI crate); keep it off in production dependency graphs.
//!
//! Prerequisites: `ssr` + `dev-tools`, signed-in session.
//!
//! ```rust,ignore
//! # async fn demo() -> Result<(), leptos::prelude::ServerFnError> {
//! use uf_notifications_api::{create_test_notification, list_notifications};
//!
//! create_test_notification(
//!     "Test title".into(),
//!     "Test body".into(),
//!     Some("/inbox".into()),
//! )
//! .await?;
//! let recent = list_notifications().await?;
//! assert!(
//!     recent.iter().any(|n| n.title == "Test title" && n.url.as_deref() == Some("/inbox")),
//!     "create_test_notification persists a recognizable row"
//! );
//! # Ok(())
//! # }
//! ```
//!
//! Persist failures map to `"notification store failed"` without Valence display text.
//! Next: workspace `examples/notifications-mount-host` for a full host walkthrough.
//!
//! ## Examples
//!
//! Start with [List notifications](#list-notifications) and [live unread badge](#live-unread-badge).
//! Unit tests in this crate cover `NotificationReadFilter`, session parse, and clamp helpers;
//! workspace `examples/notifications-mount-host` wires the API into a host. Host e2e exercises
//! mark-read and the synced unread count.
//!
//! # Feature flags
//!
//! | Feature | Default | Purpose |
//! |---------|---------|---------|
//! | `ssr` | no | Valence-backed server functions, Photon synced unread count, core re-exports |
//! | `hydrate` | no | Client hydration markers; emits real [`subscribe_get_unread_count`] |
//! | `dev-tools` | no | `create_test_notification` for local UI and e2e hosts |

#![allow(missing_docs)]
#![deny(clippy::missing_errors_doc)]

#[cfg(feature = "ssr")]
mod ops;
#[cfg(feature = "ssr")]
mod ssr_support;

mod dto;
mod read;
mod session;
mod write;

#[cfg(feature = "ssr")]
pub use ops::{
    list_for_user, mark_all_read_for_user, mark_read_for_user, mark_unread_for_user,
    notification_count_for_user, notifications_page_for_user, today_count_for_user,
    unread_count_for_user, unread_page_for_user, unread_preview_for_user, NotificationOpsError,
};

#[cfg(feature = "ssr")]
pub use uf_notifications_core::NotificationDto;

#[cfg(not(feature = "ssr"))]
pub use dto::NotificationDto;

#[cfg(feature = "ssr")]
pub use uf_notifications_core::Notification as NotificationModel;
#[cfg(feature = "ssr")]
pub use uf_notifications_core::{
    embedded_surreal, events, notification_to_dto, send_notification, SendNotification,
    SendNotificationError,
};

pub use orbital_paging::Page;
pub use read::{
    get_notification_count, get_notifications_page, get_today_count, get_unread_count,
    get_unread_notifications_page, get_unread_notifications_preview, list_notifications,
    subscribe_get_unread_count,
};
pub use session::{
    cap_notification_count, clamp_notification_page_limit, parse_session_user_id,
    truncate_notification_search, NotificationReadFilter, SessionUserIdError,
    MAX_NOTIFICATION_COUNT_CAP, MAX_NOTIFICATION_PAGE_LIMIT, MAX_NOTIFICATION_SEARCH_CHARS,
};
#[cfg(feature = "dev-tools")]
pub use write::create_test_notification;
pub use write::{mark_all_notifications_read, mark_notification_read, mark_notification_unread};

#[cfg(test)]
mod tests {
    use super::{
        cap_notification_count, clamp_notification_page_limit, parse_session_user_id,
        truncate_notification_search, NotificationReadFilter, MAX_NOTIFICATION_COUNT_CAP,
        MAX_NOTIFICATION_PAGE_LIMIT, MAX_NOTIFICATION_SEARCH_CHARS,
    };

    #[test]
    fn read_filter_variants_are_distinct() {
        assert_ne!(NotificationReadFilter::All, NotificationReadFilter::Unread);
        assert_ne!(NotificationReadFilter::Unread, NotificationReadFilter::Read);
        assert_eq!(NotificationReadFilter::All, NotificationReadFilter::All);
    }

    #[test]
    fn read_filter_serde_round_trip() {
        for filter in [
            NotificationReadFilter::All,
            NotificationReadFilter::Unread,
            NotificationReadFilter::Read,
        ] {
            let json = serde_json::to_string(&filter).expect("serialize");
            let back: NotificationReadFilter = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(back, filter);
        }
    }

    #[test]
    fn parse_session_user_id_happy_path() {
        assert_eq!(
            parse_session_user_id("user:alice").expect("valid"),
            ("user", "alice")
        );
        assert_eq!(
            parse_session_user_id("user:a1-b2").expect("valid"),
            ("user", "a1-b2")
        );
    }

    #[test]
    fn parse_session_user_id_rejects_malformed_sad_path() {
        for bad in ["", "alice", ":alice", "user:", ":", "user"] {
            let err = parse_session_user_id(bad).expect_err("must reject");
            assert_eq!(err.value, bad);
            assert!(
                err.to_string().contains("invalid session user id"),
                "unexpected error for `{bad}`: {err}"
            );
        }
    }

    #[test]
    fn clamp_notification_page_limit_caps_high_values_sad() {
        assert_eq!(clamp_notification_page_limit(0), 1);
        assert_eq!(clamp_notification_page_limit(20), 20);
        assert_eq!(
            clamp_notification_page_limit(MAX_NOTIFICATION_PAGE_LIMIT),
            MAX_NOTIFICATION_PAGE_LIMIT
        );
        assert_eq!(
            clamp_notification_page_limit(MAX_NOTIFICATION_PAGE_LIMIT + 500),
            MAX_NOTIFICATION_PAGE_LIMIT
        );
        assert_eq!(
            clamp_notification_page_limit(u32::MAX),
            MAX_NOTIFICATION_PAGE_LIMIT
        );
    }

    #[test]
    fn truncate_notification_search_caps_length_sad() {
        let over = "q".repeat(MAX_NOTIFICATION_SEARCH_CHARS + 80);
        let capped = truncate_notification_search(&over);
        assert_eq!(capped.chars().count(), MAX_NOTIFICATION_SEARCH_CHARS);
        assert_eq!(truncate_notification_search("  hello  "), "hello");
        assert_eq!(truncate_notification_search("   "), "");
    }

    #[test]
    fn cap_notification_count_stops_at_cap_sad() {
        assert_eq!(cap_notification_count(0), 0);
        assert_eq!(cap_notification_count(12), 12);
        assert_eq!(
            cap_notification_count(MAX_NOTIFICATION_COUNT_CAP as usize),
            MAX_NOTIFICATION_COUNT_CAP as usize
        );
        assert_eq!(
            cap_notification_count(MAX_NOTIFICATION_COUNT_CAP as usize + 80),
            MAX_NOTIFICATION_COUNT_CAP as usize
        );
    }
}
