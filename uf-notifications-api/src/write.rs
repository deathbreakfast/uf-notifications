//! Auth-scoped notification write paths (mark read / unread / create).

use leptos::prelude::*;
use uuid::Uuid;

#[cfg(feature = "ssr")]
use crate::ops;
#[cfg(feature = "ssr")]
use crate::ssr_support::{map_ops_err, require_auth_user, user_valence};
#[cfg(all(feature = "ssr", feature = "dev-tools"))]
use crate::{send_notification, SendNotification};

/// Mark all unread notifications as read for the current user.
///
/// Returns the number of rows that actually committed. Persist failures on
/// individual rows are skipped (and logged server-side); this call still
/// succeeds with that committed count (which may be smaller than the unread
/// query). At most [`crate::MAX_NOTIFICATION_COUNT_CAP`] unread rows are loaded.
///
/// # Errors
///
/// Returns a server-fn error when the caller is not signed in or the unread
/// query fails. Store errors do not include Valence engine text. Individual row
/// persist failures during the mark-all loop are skipped; the returned count may
/// be smaller than the unread query. [`ServerFnError`] wraps these as opaque
/// client-visible strings.
#[server]
pub async fn mark_all_notifications_read() -> Result<u32, ServerFnError> {
    let (ctx, user_id) = require_auth_user().await?;
    let v = user_valence(&ctx)?;
    ops::mark_all_read_for_user(&v, user_id)
        .await
        .map_err(map_ops_err)
}

/// Mark a notification as read
///
/// Missing rows and privacy-hidden rows complete with `Ok(())` so callers cannot
/// enumerate other users' notifications by id.
///
/// # Errors
///
/// Returns [`ServerFnError`] for the same auth failures as [`crate::list_notifications`].
/// Valence mutable/commit failures map to `"notification store failed"` without
/// engine display text. A missing or hidden notification is not an error.
///
/// # Examples
///
/// ```rust,ignore
/// use uf_notifications_api::mark_notification_read;
/// use uuid::Uuid;
///
/// let id = Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000")?;
/// mark_notification_read(id).await?;
/// ```
#[server]
pub async fn mark_notification_read(
    /// Unique id of the notification to mark as read.
    notification_id: Uuid,
) -> Result<(), ServerFnError> {
    let (ctx, _user_id) = require_auth_user().await.inspect_err(|_| {
        tracing::debug!(operation = "mark_read", outcome = "unauthenticated");
    })?;
    let v = user_valence(&ctx).inspect_err(|_| {
        tracing::debug!(operation = "mark_read", outcome = "factory");
    })?;
    ops::mark_read_for_user(&v, notification_id)
        .await
        .map_err(map_ops_err)
}

/// Mark a notification as unread
///
/// Missing rows and privacy-hidden rows complete with `Ok(())`.
///
/// # Errors
///
/// Same auth and store semantics as [`mark_notification_read`].
#[server]
pub async fn mark_notification_unread(
    /// Unique id of the notification to mark as unread.
    notification_id: Uuid,
) -> Result<(), ServerFnError> {
    let (ctx, _user_id) = require_auth_user().await?;
    let v = user_valence(&ctx)?;
    ops::mark_unread_for_user(&v, notification_id)
        .await
        .map_err(map_ops_err)
}

/// Helper server function to create test notifications (development builds only).
///
/// Gated behind the `dev-tools` feature so production SSR builds do not expose
/// an arbitrary notification-create endpoint. Valence create allows System and
/// authenticated actors (product fanout); this helper mints a row for the
/// **authenticated session user** via [`crate::send_notification`] (same persist
/// + Photon publish path as backend callers).
///
/// Enable `dev-tools` on both `uf-notifications-api` and `uf-notifications` if
/// you call this from the UI `server` re-export module. Keep the feature off
/// in production dependency graphs.
///
/// # Errors
///
/// Same auth failures as [`crate::list_notifications`]. Persist/construct
/// failures from [`crate::send_notification`] map to `"notification store failed"`
/// (the underlying [`crate::SendNotificationError`] display text is not forwarded).
#[cfg(feature = "dev-tools")]
#[server]
pub async fn create_test_notification(
    /// Notification title.
    title: String,
    /// Notification body text.
    message: String,
    /// Optional link the client should navigate to when the notification is clicked.
    url: Option<String>,
) -> Result<(), ServerFnError> {
    let (ctx, user_id) = require_auth_user().await?;
    let v = user_valence(&ctx)?;

    send_notification(
        SendNotification {
            user_id,
            kind: "general".into(),
            title,
            message,
            url,
            data_json: None,
        },
        &v,
    )
    .await
    .map_err(|_| ServerFnError::new("notification store failed"))?;

    Ok(())
}
