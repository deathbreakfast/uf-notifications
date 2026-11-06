//! Audience checks that decide who may receive a scoped notification.
//!
//! A scoped notification names a `subject` record (`table:id`). When a
//! non-System actor creates one, [`crate::SCOPED_NOTIFICATION_CREATE`] looks up
//! the audience check registered for the subject's table (and optionally the
//! notification `kind`) and asks it whether the recipient belongs to that
//! subject's audience.
//!
//! # Rules for checks
//!
//! - A check answers yes or no for **one** recipient. It never returns, logs,
//!   or traces the audience list or other members' ids.
//! - It runs with the caller's own [`valence::Valence`], so every read it makes
//!   is under the sending actor's privacy. Never elevate to `Actor::System`
//!   inside a check.
//! - An `Err` from a check denies the create.
//! - With no check registered for a subject table, the create is allowed only
//!   when the recipient is the actor itself.
//!
//! The registry is process-wide. Hosts register checks once at boot, before
//! serving requests; a later registration for the same key replaces the earlier
//! one and logs a `warn`.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use futures::future::BoxFuture;

/// Async audience check: `(subject, recipient, valence) -> is the recipient in
/// the subject's audience?`
///
/// `subject` is the full `table:id` string from the notification row and
/// `recipient` is the recipient's user id without a `user:` prefix.
pub type NotificationAudienceCheck = Arc<
    dyn for<'a> Fn(&'a str, &'a str, &'a valence::Valence) -> BoxFuture<'a, valence::Result<bool>>
        + Send
        + Sync,
>;

type RegistryKey = (String, Option<String>);

fn registry() -> &'static RwLock<HashMap<RegistryKey, NotificationAudienceCheck>> {
    static REGISTRY: OnceLock<RwLock<HashMap<RegistryKey, NotificationAudienceCheck>>> =
        OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Register the audience check for notifications whose subject lives in `table`.
///
/// Pass `Some(kind)` to scope the check to one notification kind; `None`
/// covers every kind for that table. A kind-specific check wins over the
/// table-wide one. Call this at host boot. Registering the same key again
/// replaces the earlier check and logs a `warn`.
///
/// # Examples
///
/// ```rust
/// use uf_notifications_core::register_notification_audience;
///
/// // Members of a team may be notified about that team.
/// register_notification_audience("team", None, |subject, recipient, valence| {
///     Box::pin(async move {
///         // Look up membership with `valence` (the caller's actor, never System)
///         // and answer for this one recipient only.
///         let _ = (subject, recipient, valence);
///         Ok(false)
///     })
/// });
/// ```
pub fn register_notification_audience<F>(table: &str, kind: Option<&str>, check: F)
where
    F: for<'a> Fn(&'a str, &'a str, &'a valence::Valence) -> BoxFuture<'a, valence::Result<bool>>
        + Send
        + Sync
        + 'static,
{
    let key = (table.to_string(), kind.map(str::to_string));
    let mut map = registry()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if map.insert(key, Arc::new(check)).is_some() {
        tracing::warn!(
            operation = "register_audience",
            table,
            kind = kind.unwrap_or("*"),
            "notification audience check replaced an earlier registration"
        );
    }
}

/// The check for `(table, kind)`, falling back to the table-wide check.
pub(crate) fn lookup_audience_check(table: &str, kind: &str) -> Option<NotificationAudienceCheck> {
    let map = registry()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.get(&(table.to_string(), Some(kind.to_string())))
        .or_else(|| map.get(&(table.to_string(), None)))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::{lookup_audience_check, register_notification_audience};

    #[test]
    fn kind_specific_check_wins_and_table_wide_falls_back() {
        register_notification_audience("audience_unit_t", None, |_, _, _| {
            Box::pin(async { Ok(false) })
        });
        register_notification_audience("audience_unit_t", Some("special"), |_, _, _| {
            Box::pin(async { Ok(true) })
        });
        assert!(lookup_audience_check("audience_unit_t", "special").is_some());
        assert!(lookup_audience_check("audience_unit_t", "other").is_some());
        assert!(lookup_audience_check("audience_unit_missing", "special").is_none());
    }
}
