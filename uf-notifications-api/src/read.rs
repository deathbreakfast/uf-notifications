//! Auth-scoped notification read paths (`list` / page / counts).

use leptos::prelude::*;
use orbital_paging::{Page, PageRequest};

use crate::{
    KindDto, NotificationDto, NotificationInboxFilter, NotificationReadFilter, PreferencesDto,
};

#[cfg(feature = "ssr")]
use crate::ops;
#[cfg(feature = "ssr")]
use crate::ssr_support::{map_ops_err, require_auth_user, user_valence};

/// List all notifications for the current user
///
/// # Errors
///
/// Returns [`ServerFnError`] when the caller is not signed in (`"You must be signed in"`),
/// the session user id is malformed (`"invalid session user id"`), Higgs context is
/// missing (`"request context failed"`), or the Valence query fails (`"notification store failed"`).
/// Valence engine display text is not forwarded. [`ServerFnError`] wraps these as opaque
/// client-visible strings.
///
/// # Examples
///
/// ```rust,ignore
/// use uf_notifications_api::list_notifications;
///
/// let recent = list_notifications().await?;
/// ```
#[server]
pub async fn list_notifications() -> Result<Vec<NotificationDto>, ServerFnError> {
    let (ctx, user_id) = require_auth_user().await?;
    let v = user_valence(&ctx)?;
    ops::list_for_user(&v, user_id).await.map_err(map_ops_err)
}

/// Get unread notifications preview (for bell dropdown)
///
/// # Errors
///
/// Same auth and store semantics as [`list_notifications`].
#[server]
pub async fn get_unread_notifications_preview() -> Result<Vec<NotificationDto>, ServerFnError> {
    let (ctx, user_id) = require_auth_user().await?;
    let v = user_valence(&ctx)?;
    ops::unread_preview_for_user(&v, user_id)
        .await
        .map_err(map_ops_err)
}

/// Get a page of unread notifications (for bell dropdown with infinite scroll).
///
/// # Errors
///
/// Same auth and store semantics as [`list_notifications`]. `offset` and `limit` are
/// clamped server-side; invalid values do not produce a separate error.
#[server]
pub async fn get_unread_notifications_page(
    /// Zero-based index of the first notification to return.
    offset: u32,
    /// Maximum number of notifications to return.
    limit: u32,
) -> Result<Page<NotificationDto>, ServerFnError> {
    let (ctx, user_id) = require_auth_user().await?;
    let v = user_valence(&ctx)?;
    ops::unread_page_for_user(&v, user_id, offset, limit)
        .await
        .map_err(map_ops_err)
}

/// Get a page of unread notifications from a [`PageRequest`], continuing from its cursor.
///
/// Pass the previous page's [`Page::next_cursor`] as `request.cursor` (as
/// `orbital_paging::use_paged_infinite_scroll` does) and the server resumes
/// where that page stopped instead of re-reading every earlier row. Without a
/// cursor, or with one from another list, it pages by `request.offset` like
/// [`get_unread_notifications_page`]. Cursor pages return `total_count: None`.
///
/// # Errors
///
/// Same auth and store semantics as [`list_notifications`]. An unusable cursor
/// falls back to `offset` and does not produce an error.
#[server]
pub async fn get_unread_notifications_by_request(
    /// Offset, limit and cursor; sort, filter and quick search are ignored.
    request: PageRequest,
) -> Result<Page<NotificationDto>, ServerFnError> {
    let (ctx, user_id) = require_auth_user().await?;
    let v = user_valence(&ctx)?;
    ops::unread_page_for_request(&v, user_id, &request)
        .await
        .map_err(map_ops_err)
}

/// Get count of unread notifications.
///
/// Loads at most [`crate::MAX_NOTIFICATION_COUNT_CAP`] rows and returns that
/// length. Larger inboxes report the cap.
///
/// When `ssr` or `hydrate` is enabled, `#[photon_leptos::synced]` subscribes to
/// topic `user.notifications` on `/ws/notifications` and emits
/// [`subscribe_get_unread_count`] so clients refetch after `NotificationPushed`.
///
/// # Errors
///
/// Same auth and store semantics as [`list_notifications`].
///
/// # Examples
///
/// ```rust,ignore
/// use uf_notifications_api::get_unread_count;
///
/// // Signed-in client; host must mount Photon WS at /ws/notifications.
/// let count = get_unread_count().await?;
/// ```
#[cfg_attr(
    any(feature = "ssr", feature = "hydrate"),
    photon_leptos::synced(
        topic = "user.notifications",
        ws = "/ws/notifications",
        strategy = "refetch",
        auth = "user"
    )
)]
#[server]
pub async fn get_unread_count() -> Result<usize, ServerFnError> {
    let (ctx, user_id) = require_auth_user().await?;
    let v = user_valence(&ctx)?;
    ops::unread_count_for_user(&v, user_id)
        .await
        .map_err(map_ops_err)
}

/// Fallback subscription helper for builds with neither `ssr` nor `hydrate`.
///
/// When either feature is enabled, `#[photon_leptos::synced]` on
/// [`get_unread_count`] generates the real [`subscribe_get_unread_count`]
/// helper. In feature-less builds the macro is compiled out entirely, so this
/// stub keeps the API stable: the trigger stays at `0` and the callback is
/// never invoked (matching the macro's no-WebSocket behavior).
///
/// The bell wires this trigger to a `Resource` that calls [`get_unread_count`];
/// each Photon push on `user.notifications` bumps the trigger and refetches.
#[cfg(not(any(feature = "ssr", feature = "hydrate")))]
pub fn subscribe_get_unread_count(_on_event: impl Fn() + Send + Sync + 'static) -> RwSignal<u64> {
    RwSignal::new(0u64)
}

/// Get count of notifications created today
///
/// # Errors
///
/// Same auth and store semantics as [`list_notifications`], plus
/// `"invalid local midnight"` when the host cannot construct today's start
/// timestamp (extremely rare platform edge).
#[server]
pub async fn get_today_count() -> Result<usize, ServerFnError> {
    let (ctx, user_id) = require_auth_user().await?;
    let v = user_valence(&ctx)?;
    ops::today_count_for_user(&v, user_id)
        .await
        .map_err(map_ops_err)
}

/// Get a page of notifications for the current user with server-side search and read-status filtering.
///
/// `read_filter` and `query` are applied server-side; they do not produce
/// separate validation errors. Search text is truncated to
/// [`crate::MAX_NOTIFICATION_SEARCH_CHARS`]; `limit` is clamped via
/// [`crate::clamp_notification_page_limit`].
///
/// # Errors
///
/// Same auth and store semantics as [`list_notifications`].
///
/// # Examples
///
/// ```rust,ignore
/// use uf_notifications_api::{get_notifications_page, NotificationReadFilter};
///
/// let page = get_notifications_page(
///     0,
///     20,
///     Some("leaderboard".into()),
///     NotificationReadFilter::Unread,
/// )
/// .await?;
/// ```
#[server]
pub async fn get_notifications_page(
    /// Zero-based index of the first notification to return.
    offset: u32,
    /// Maximum number of notifications to return.
    limit: u32,
    /// Optional free-text search matched against title and message.
    query: Option<String>,
    /// Read-status filter to apply.
    read_filter: NotificationReadFilter,
) -> Result<Page<NotificationDto>, ServerFnError> {
    let (ctx, user_id) = require_auth_user().await?;
    let v = user_valence(&ctx)?;
    ops::notifications_page_for_user(&v, user_id, offset, limit, query, read_filter)
        .await
        .map_err(map_ops_err)
}

/// Get a page of notifications with search and read filter from a [`PageRequest`],
/// continuing from its cursor.
///
/// Same rows as [`get_notifications_page`]. Pass the previous page's
/// [`Page::next_cursor`] as `request.cursor` and the server resumes where that
/// page stopped. A cursor issued under another `query` or `read_filter` is
/// ignored and the page is read from `request.offset`. Cursor pages return
/// `total_count: None`.
///
/// # Errors
///
/// Same auth and store semantics as [`list_notifications`]. An unusable cursor
/// falls back to `offset` and does not produce an error.
#[server]
pub async fn get_notifications_by_request(
    /// Offset, limit and cursor; sort, filter and quick search are ignored.
    request: PageRequest,
    /// Optional free-text search matched against title and message.
    query: Option<String>,
    /// Read-status filter to apply.
    read_filter: NotificationReadFilter,
) -> Result<Page<NotificationDto>, ServerFnError> {
    let (ctx, user_id) = require_auth_user().await?;
    let v = user_valence(&ctx)?;
    ops::notifications_page_for_request(&v, user_id, &request, query, read_filter)
        .await
        .map_err(map_ops_err)
}

/// Get total notification count for the current user.
///
/// # Errors
///
/// Same auth and store semantics as [`list_notifications`].
#[server]
pub async fn get_notification_count() -> Result<usize, ServerFnError> {
    let (ctx, user_id) = require_auth_user().await?;
    let v = user_valence(&ctx)?;
    ops::notification_count_for_user(&v, user_id)
        .await
        .map_err(map_ops_err)
}

/// Get a page of the inbox filtered to kinds that ask for action and/or one app.
///
/// Same rows as [`get_notifications_by_request`], narrowed by `filter`:
/// `asks_action` keeps kinds that ask the reader to act and `app_id` keeps one
/// app's kinds, both from the kind registry. Unregistered kinds never match an
/// active filter. A default filter returns the same page as
/// [`get_notifications_by_request`].
///
/// # Errors
///
/// Same auth and store semantics as [`list_notifications`]. An unusable cursor
/// falls back to `offset` and does not produce an error.
#[server]
pub async fn get_inbox_by_request(
    /// Offset, limit and cursor; sort, filter and quick search are ignored.
    request: PageRequest,
    /// Optional free-text search matched against title and message.
    query: Option<String>,
    /// Read-status filter to apply.
    read_filter: NotificationReadFilter,
    /// Asks-for-action and app filter.
    filter: NotificationInboxFilter,
) -> Result<Page<NotificationDto>, ServerFnError> {
    let (ctx, user_id) = require_auth_user().await?;
    let v = user_valence(&ctx)?;
    ops::inbox_page_for_request(&v, user_id, &request, query, read_filter, &filter)
        .await
        .map_err(map_ops_err)
}

/// List every registered notification kind with its default channels,
/// grouped by app (sorted by app, then label).
///
/// # Errors
///
/// Returns [`ServerFnError`] when the caller is not signed in, like
/// [`list_notifications`]. The registry itself cannot fail.
#[server]
pub async fn list_notification_kinds() -> Result<Vec<KindDto>, ServerFnError> {
    let _ = require_auth_user().await?;
    Ok(uf_notifications_core::kinds()
        .into_iter()
        .map(KindDto::from)
        .collect())
}

/// Get the signed-in person's notification preferences: every registered kind
/// with its effective channels, their digest and quiet-hours settings, and
/// whether their email and phone are verified.
///
/// # Errors
///
/// Same auth semantics as [`list_notifications`]; a failed preference read or
/// verification check maps to `"notification store failed"`.
#[server]
pub async fn get_notification_preferences() -> Result<PreferencesDto, ServerFnError> {
    let (ctx, user_id) = require_auth_user().await?;
    let v = user_valence(&ctx)?;
    preferences_for_user(&v, &user_id)
        .await
        .map_err(map_ops_err)
}

/// [`get_notification_preferences`] for `user` as the actor on `valence`
/// (testable without Higgs). Reads as that actor; never elevates.
///
/// # Errors
///
/// Returns [`ops::NotificationOpsError::Store`] when a preference read or the
/// host's verification check fails.
#[cfg(feature = "ssr")]
pub async fn preferences_for_user(
    valence: &valence::Valence,
    user: &valence::RecordId,
) -> Result<PreferencesDto, ops::NotificationOpsError> {
    use std::collections::HashMap;
    use uf_notifications_core::{
        contact_verified, load_preferences, load_settings, mask_unverified,
    };

    let store = |_| ops::NotificationOpsError::Store;
    let saved: HashMap<String, uf_notifications_core::Channels> = load_preferences(valence, user)
        .await
        .map_err(store)?
        .into_iter()
        .collect();
    let settings = load_settings(valence, user).await.map_err(store)?;
    let (email_verified, text_verified) = contact_verified(user, valence).await.map_err(store)?;
    let kinds = uf_notifications_core::kinds()
        .into_iter()
        .map(|k| {
            let stored = saved.get(k.id).copied().unwrap_or(k.defaults);
            crate::KindPreferenceDto {
                kind: KindDto::from(k),
                channels: mask_unverified(stored, email_verified, text_verified).into(),
            }
        })
        .collect();
    Ok(PreferencesDto {
        kinds,
        settings: settings.into(),
        email_verified,
        text_verified,
    })
}
