//! Valence-injected notification query/mark helpers (testable without Higgs Owner).
//!
//! `#[server]` wrappers in [`crate::read`] / [`crate::write`] call these after
//! [`crate::ssr_support::require_auth_user`], then map [`NotificationOpsError`]
//! to opaque [`leptos::prelude::ServerFnError`] via
//! [`crate::ssr_support::map_ops_err`].

use chrono::{DateTime, Utc};
use orbital_paging::Page;
use tracing::Instrument;
use uuid::Uuid;
use valence::{
    DateTimePredicate, Model, RecordId, RecordPredicate, SortDirection, StringPredicate, Valence,
};

use crate::{
    cap_notification_count, clamp_notification_page_limit, notification_to_dto,
    truncate_notification_search, NotificationDto, NotificationModel, NotificationReadFilter,
    MAX_NOTIFICATION_COUNT_CAP,
};

/// Typed failure from Valence-backed notification ops (before the Leptos boundary).
///
/// Client-facing strings are produced only by [`crate::ssr_support::map_ops_err`].
/// Variants do not embed Valence engine Display text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationOpsError {
    /// Valence query, get, mutable, or commit failed.
    Store,
    /// UTC midnight for "today" could not be constructed.
    InvalidMidnight,
}

impl std::fmt::Display for NotificationOpsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store => write!(f, "notification store failed"),
            Self::InvalidMidnight => write!(f, "invalid local midnight"),
        }
    }
}

impl std::error::Error for NotificationOpsError {}

fn store_err(_err: valence::Error) -> NotificationOpsError {
    NotificationOpsError::Store
}

/// Treat privacy-hidden and missing rows as `None` so mark-read cannot enumerate peers.
///
/// # Errors
///
/// Returns [`NotificationOpsError::Store`] for Valence errors other than privacy /
/// not-found / pending-deletion (those become `Ok(None)`).
pub(crate) fn optional_notification(
    result: Result<Option<NotificationModel>, valence::Error>,
) -> Result<Option<NotificationModel>, NotificationOpsError> {
    match result {
        Ok(row) => Ok(row),
        Err(
            valence::Error::Privacy(_)
            | valence::Error::NotFound(_)
            | valence::Error::PendingDeletion(_),
        ) => Ok(None),
        Err(_) => Err(NotificationOpsError::Store),
    }
}

fn read_filter_label(filter: NotificationReadFilter) -> &'static str {
    match filter {
        NotificationReadFilter::All => "all",
        NotificationReadFilter::Unread => "unread",
        NotificationReadFilter::Read => "read",
    }
}

/// List up to 50 recent notifications for `user_id` (newest first).
///
/// # Errors
///
/// Returns [`NotificationOpsError::Store`] on Valence errors.
pub async fn list_for_user(
    valence: &Valence,
    user_id: RecordId,
) -> Result<Vec<NotificationDto>, NotificationOpsError> {
    async {
        let notifications: Vec<NotificationModel> = NotificationModel::query(valence, valence::use_!(r"In **notifications**, we **list Notification Model** so the product can show or process the matching set for this workflow. Callers allowed for **notifications** use the list; it is not a public dump of every field to anonymous visitors."))
            .where_user(RecordPredicate::Equals(user_id))
            .order_by_created_at(SortDirection::Desc)
            .limit(50)
            .await
            .map_err(store_err)?;

        let dtos: Vec<NotificationDto> = notifications.iter().map(notification_to_dto).collect();
        tracing::Span::current().record("result_len", dtos.len());
        Ok(dtos)
    }
    .instrument(tracing::info_span!(
        "uf_notifications.list",
        operation = "list",
        result_len = tracing::field::Empty,
    ))
    .await
}

/// Newest unread preview (max 10) for the bell dropdown.
///
/// # Errors
///
/// Returns [`NotificationOpsError::Store`] on Valence errors.
pub async fn unread_preview_for_user(
    valence: &Valence,
    user_id: RecordId,
) -> Result<Vec<NotificationDto>, NotificationOpsError> {
    async {
        let notifications: Vec<NotificationModel> = NotificationModel::query(valence, valence::use_!(r"In **notifications**, we **list Notification Model** so the product can show or process the matching set for this workflow. Callers allowed for **notifications** use the list; it is not a public dump of every field to anonymous visitors."))
            .where_user(RecordPredicate::Equals(user_id))
            .where_read_at_is_none()
            .order_by_created_at(SortDirection::Desc)
            .limit(10)
            .await
            .map_err(store_err)?;

        let dtos: Vec<NotificationDto> = notifications.iter().map(notification_to_dto).collect();
        tracing::Span::current().record("result_len", dtos.len());
        Ok(dtos)
    }
    .instrument(tracing::info_span!(
        "uf_notifications.unread_preview",
        operation = "unread_preview",
        result_len = tracing::field::Empty,
    ))
    .await
}

/// Paged unread rows for bell infinite scroll.
///
/// # Errors
///
/// Returns [`NotificationOpsError::Store`] on Valence errors.
pub async fn unread_page_for_user(
    valence: &Valence,
    user_id: RecordId,
    offset: u32,
    limit: u32,
) -> Result<Page<NotificationDto>, NotificationOpsError> {
    let limit = clamp_notification_page_limit(limit);

    async {
        let notifications: Vec<NotificationModel> = NotificationModel::query(valence, valence::use_!(r"In **notifications**, we **list Notification Model** so the product can show or process the matching set for this workflow. Callers allowed for **notifications** use the list; it is not a public dump of every field to anonymous visitors."))
            .where_user(RecordPredicate::Equals(user_id.clone()))
            .where_read_at_is_none()
            .order_by_created_at(SortDirection::Desc)
            .limit(limit + 1)
            .offset(offset)
            .await
            .map_err(store_err)?;

        let total_count: Option<u64> = if offset == 0 {
            let all_unread: Vec<NotificationModel> = NotificationModel::query(valence, valence::use_!(r"In **notifications**, we **list Notification Model** so the product can show or process the matching set for this workflow. Callers allowed for **notifications** use the list; it is not a public dump of every field to anonymous visitors."))
                .where_user(RecordPredicate::Equals(user_id))
                .where_read_at_is_none()
                .limit(MAX_NOTIFICATION_COUNT_CAP)
                .await
                .map_err(store_err)?;
            Some(cap_notification_count(all_unread.len()) as u64)
        } else {
            None
        };

        let dtos: Vec<NotificationDto> = notifications.iter().map(notification_to_dto).collect();
        tracing::Span::current().record("result_len", dtos.len());
        Ok(Page::from_oversized(dtos, limit, total_count))
    }
    .instrument(tracing::info_span!(
        "uf_notifications.unread_page",
        operation = "unread_page",
        offset,
        limit,
        result_len = tracing::field::Empty,
    ))
    .await
}

/// Unread count (capped).
///
/// # Errors
///
/// Returns [`NotificationOpsError::Store`] on Valence errors.
pub async fn unread_count_for_user(
    valence: &Valence,
    user_id: RecordId,
) -> Result<usize, NotificationOpsError> {
    async {
        let notifications: Vec<NotificationModel> = NotificationModel::query(valence, valence::use_!(r"In **notifications**, we **list Notification Model** so the product can show or process the matching set for this workflow. Callers allowed for **notifications** use the list; it is not a public dump of every field to anonymous visitors."))
            .where_user(RecordPredicate::Equals(user_id))
            .where_read_at_is_none()
            .limit(MAX_NOTIFICATION_COUNT_CAP)
            .await
            .map_err(store_err)?;

        let count = cap_notification_count(notifications.len());
        tracing::Span::current().record("result_len", count);
        Ok(count)
    }
    .instrument(tracing::info_span!(
        "uf_notifications.unread_count",
        operation = "unread_count",
        result_len = tracing::field::Empty,
    ))
    .await
}

/// Count of notifications created at or after local UTC midnight today.
///
/// # Errors
///
/// Returns [`NotificationOpsError::InvalidMidnight`] when midnight cannot be
/// constructed, or [`NotificationOpsError::Store`] on Valence errors.
pub async fn today_count_for_user(
    valence: &Valence,
    user_id: RecordId,
) -> Result<usize, NotificationOpsError> {
    async {
        let Some(midnight) = Utc::now().date_naive().and_hms_opt(0, 0, 0) else {
            return Err(NotificationOpsError::InvalidMidnight);
        };
        let today_start = DateTime::<Utc>::from_naive_utc_and_offset(midnight, Utc);

        let notifications: Vec<NotificationModel> = NotificationModel::query(valence, valence::use_!(r"In **notifications**, we **list Notification Model** so the product can show or process the matching set for this workflow. Callers allowed for **notifications** use the list; it is not a public dump of every field to anonymous visitors."))
            .where_user(RecordPredicate::Equals(user_id))
            .where_created_at(DateTimePredicate::After(today_start))
            .limit(MAX_NOTIFICATION_COUNT_CAP)
            .await
            .map_err(store_err)?;

        let count = cap_notification_count(notifications.len());
        tracing::Span::current().record("result_len", count);
        Ok(count)
    }
    .instrument(tracing::info_span!(
        "uf_notifications.today_count",
        operation = "today_count",
        result_len = tracing::field::Empty,
    ))
    .await
}

/// Total notification count for the user (capped).
///
/// # Errors
///
/// Returns [`NotificationOpsError::Store`] on Valence errors.
pub async fn notification_count_for_user(
    valence: &Valence,
    user_id: RecordId,
) -> Result<usize, NotificationOpsError> {
    async {
        let notifications: Vec<NotificationModel> = NotificationModel::query(valence, valence::use_!(r"In **notifications**, we **list Notification Model** so the product can show or process the matching set for this workflow. Callers allowed for **notifications** use the list; it is not a public dump of every field to anonymous visitors."))
            .where_user(RecordPredicate::Equals(user_id))
            .limit(MAX_NOTIFICATION_COUNT_CAP)
            .await
            .map_err(store_err)?;

        let count = cap_notification_count(notifications.len());
        tracing::Span::current().record("result_len", count);
        Ok(count)
    }
    .instrument(tracing::info_span!(
        "uf_notifications.notification_count",
        operation = "notification_count",
        result_len = tracing::field::Empty,
    ))
    .await
}

/// Inbox page with search + read filter.
///
/// # Errors
///
/// Returns [`NotificationOpsError::Store`] on Valence errors.
pub async fn notifications_page_for_user(
    valence: &Valence,
    user_id: RecordId,
    offset: u32,
    limit: u32,
    query: Option<String>,
    read_filter: NotificationReadFilter,
) -> Result<Page<NotificationDto>, NotificationOpsError> {
    let limit = clamp_notification_page_limit(limit);
    let filter = read_filter_label(read_filter);
    let search_term = query
        .as_deref()
        .map(truncate_notification_search)
        .filter(|s| !s.is_empty());
    let has_query = search_term.is_some();

    async {
        let base = {
            let mut q = NotificationModel::query(valence, valence::use_!(r"In **notifications**, we **list Notification Model** so the product can show or process the matching set for this workflow. Callers allowed for **notifications** use the list; it is not a public dump of every field to anonymous visitors."))
                .where_user(RecordPredicate::Equals(user_id.clone()));
            match read_filter {
                NotificationReadFilter::Unread => q = q.where_read_at_is_none(),
                NotificationReadFilter::Read => q = q.where_read_at_is_some(),
                NotificationReadFilter::All => {}
            }
            q
        };

        let filtered = if let Some(ref term) = search_term {
            let by_title = base
                .clone()
                .where_title(StringPredicate::Contains(term.clone()));
            let by_message = base
                .clone()
                .where_message(StringPredicate::Contains(term.clone()));
            by_title.union(by_message)
        } else {
            base.clone()
        };

        let notifications: Vec<NotificationModel> = filtered
            .clone()
            .order_by_created_at(SortDirection::Desc)
            .limit(limit + 1)
            .offset(offset)
            .await
            .map_err(store_err)?;

        let total_count: Option<u64> = if offset == 0 {
            let all: Vec<NotificationModel> = filtered
                .limit(MAX_NOTIFICATION_COUNT_CAP)
                .await
                .map_err(store_err)?;
            Some(cap_notification_count(all.len()) as u64)
        } else {
            None
        };

        let dtos: Vec<NotificationDto> = notifications.iter().map(notification_to_dto).collect();
        tracing::Span::current().record("result_len", dtos.len());
        Ok(Page::from_oversized(dtos, limit, total_count))
    }
    .instrument(tracing::info_span!(
        "uf_notifications.page",
        operation = "page",
        offset,
        limit,
        filter,
        has_query,
        result_len = tracing::field::Empty,
    ))
    .await
}

/// Mark all unread notifications read; returns committed count.
///
/// Loads at most [`MAX_NOTIFICATION_COUNT_CAP`] unread rows. Individual row
/// persist failures are skipped and logged at `warn` with `error_class` (no
/// Valence Display / PII); the returned count may be smaller than the query.
///
/// # Errors
///
/// Returns [`NotificationOpsError::Store`] when the unread query fails.
pub async fn mark_all_read_for_user(
    valence: &Valence,
    user_id: RecordId,
) -> Result<u32, NotificationOpsError> {
    async {
        let unread: Vec<NotificationModel> = NotificationModel::query(valence, valence::use_!(r"In **notifications**, we **list Notification Model** so the product can show or process the matching set for this workflow. Callers allowed for **notifications** use the list; it is not a public dump of every field to anonymous visitors."))
            .where_user(RecordPredicate::Equals(user_id))
            .where_read_at_is_none()
            .limit(MAX_NOTIFICATION_COUNT_CAP)
            .await
            .map_err(store_err)?;

        let now = Utc::now();
        let mut committed: u32 = 0;
        let mut rows_skipped: u32 = 0;

        for notification in unread {
            let id_str = notification
                .id()
                .map(|t| {
                    let s = t.to_string();
                    s.split(':')
                        .next_back()
                        .unwrap_or(&s)
                        .trim_matches(|c| c == '⟨' || c == '⟩')
                        .to_string()
                })
                .unwrap_or_default();

            if id_str.is_empty() {
                rows_skipped = rows_skipped.saturating_add(1);
                tracing::warn!(
                    operation = "mark_all",
                    outcome = "row_skip",
                    error_class = "missing_id"
                );
                continue;
            }
            match notification.get_mutable(valence, valence::use_!(r"In **notifications**, we **update this data** so later steps see the latest values for this workflow. Callers allowed for **notifications** use the updated data; this is not a public export of unrelated fields.")).set_read_at(now) {
                Ok(mutable) => match mutable.commit().await {
                    Ok(_) => {
                        committed = committed.saturating_add(1);
                    }
                    Err(_) => {
                        rows_skipped = rows_skipped.saturating_add(1);
                        tracing::warn!(
                            operation = "mark_all",
                            outcome = "row_skip",
                            error_class = "commit"
                        );
                    }
                },
                Err(_) => {
                    rows_skipped = rows_skipped.saturating_add(1);
                    tracing::warn!(
                        operation = "mark_all",
                        outcome = "row_skip",
                        error_class = "set_read_at"
                    );
                }
            }
        }

        tracing::Span::current().record("result_len", committed);
        tracing::Span::current().record("rows_skipped", rows_skipped);
        tracing::Span::current()
            .record("outcome", if rows_skipped == 0 { "ok" } else { "partial" });
        Ok(committed)
    }
    .instrument(tracing::info_span!(
        "uf_notifications.mark_all",
        operation = "mark_all",
        outcome = tracing::field::Empty,
        result_len = tracing::field::Empty,
        rows_skipped = tracing::field::Empty,
    ))
    .await
}

/// Mark one notification read. Missing/hidden rows → `Ok(())`.
///
/// # Errors
///
/// Returns [`NotificationOpsError::Store`] on store errors other than
/// privacy/not-found.
pub async fn mark_read_for_user(
    valence: &Valence,
    notification_id: Uuid,
) -> Result<(), NotificationOpsError> {
    async {
        let id_str = notification_id.to_string();
        let maybe_notification: Option<NotificationModel> =
            optional_notification(NotificationModel::get(&id_str, valence, valence::use_!(r"In **notifications**, we **load Notification Model** so the application can decide what to do next in this workflow. The result is used by **notifications** logic—not necessarily displayed on a page unless that feature’s UI shows it.")).await)?;

        if let Some(notification) = maybe_notification {
            notification
                .get_mutable(valence, valence::use_!(r"In **notifications**, we **update this data** so later steps see the latest values for this workflow. Callers allowed for **notifications** use the updated data; this is not a public export of unrelated fields."))
                .set_read_at(Utc::now())
                .map_err(store_err)?
                .commit()
                .await
                .map_err(store_err)?;
            tracing::Span::current().record("outcome", "updated");
        } else {
            tracing::Span::current().record("outcome", "not_found");
        }
        Ok(())
    }
    .instrument(tracing::info_span!(
        "uf_notifications.mark_read",
        operation = "mark_read",
        outcome = tracing::field::Empty,
    ))
    .await
}

/// Mark one notification unread. Missing/hidden rows → `Ok(())`.
///
/// # Errors
///
/// Same store semantics as [`mark_read_for_user`].
pub async fn mark_unread_for_user(
    valence: &Valence,
    notification_id: Uuid,
) -> Result<(), NotificationOpsError> {
    async {
        let id_str = notification_id.to_string();
        let maybe_notification: Option<NotificationModel> =
            optional_notification(NotificationModel::get(&id_str, valence, valence::use_!(r"In **notifications**, we **load Notification Model** so the application can decide what to do next in this workflow. The result is used by **notifications** logic—not necessarily displayed on a page unless that feature’s UI shows it.")).await)?;

        if let Some(notification) = maybe_notification {
            notification
                .get_mutable(valence, valence::use_!(r"In **notifications**, we **update this data** so later steps see the latest values for this workflow. Callers allowed for **notifications** use the updated data; this is not a public export of unrelated fields."))
                .clear_read_at()
                .commit()
                .await
                .map_err(store_err)?;
            tracing::Span::current().record("outcome", "updated");
        } else {
            tracing::Span::current().record("outcome", "not_found");
        }
        Ok(())
    }
    .instrument(tracing::info_span!(
        "uf_notifications.mark_unread",
        operation = "mark_unread",
        outcome = tracing::field::Empty,
    ))
    .await
}
