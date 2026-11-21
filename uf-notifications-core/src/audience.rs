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
//! # Where checks live
//!
//! Checks live in a [`NotificationAudiences`] registry that the policy reads
//! from the acting Valence through [`valence::Valence::extension`]. A host
//! installs one with [`NotificationAudiences::install`] when it builds Valence;
//! every handle made from it with `with_actor` sees the same registry.
//!
//! [`register_notification_audience`] fills the default registry,
//! [`NotificationAudiences::shared`]. The policy uses that one only when the
//! host installed none, so a host that installs `shared()` (or its own
//! registry) decides which checks apply. Registering the same key again
//! replaces the earlier check and logs a `warn`.

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

/// Audience checks keyed by subject table and, optionally, notification kind.
///
/// Install one on Valence with [`NotificationAudiences::install`] so
/// [`crate::SCOPED_NOTIFICATION_CREATE`] reads it through
/// [`valence::Valence::extension`].
///
/// # Examples
///
/// ```rust
/// use std::sync::Arc;
/// use uf_notifications_core::NotificationAudiences;
/// use valence::Valence;
///
/// let audiences = Arc::new(NotificationAudiences::new());
/// audiences.register("team", None, |subject, recipient, valence| {
///     Box::pin(async move {
///         // Answer for this one recipient, reading with the caller's `valence`.
///         let _ = (subject, recipient, valence);
///         Ok(false)
///     })
/// });
/// let builder = audiences.install(Valence::builder());
/// # let _ = builder;
/// ```
#[derive(Default)]
pub struct NotificationAudiences {
    checks: RwLock<HashMap<RegistryKey, NotificationAudienceCheck>>,
}

impl NotificationAudiences {
    /// An empty registry: every scoped create to someone other than the actor
    /// is refused until a check is registered.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The default registry that [`register_notification_audience`] fills.
    ///
    /// The policy falls back to it when the acting Valence has no
    /// `NotificationAudiences` extension. A host can install this same handle
    /// so checks registered later (for example lazily on first send) still
    /// apply.
    #[must_use]
    pub fn shared() -> Arc<Self> {
        static SHARED: OnceLock<Arc<NotificationAudiences>> = OnceLock::new();
        Arc::clone(SHARED.get_or_init(|| Arc::new(Self::new())))
    }

    /// Put this registry on `builder` as a Valence extension.
    #[must_use]
    pub fn install(self: Arc<Self>, builder: valence::ValenceBuilder) -> valence::ValenceBuilder {
        builder.extension(self)
    }

    /// Register the audience check for notifications whose subject lives in
    /// `table`. See [`register_notification_audience`] for the key rules.
    pub fn register<F>(&self, table: &str, kind: Option<&str>, check: F)
    where
        F: for<'a> Fn(
                &'a str,
                &'a str,
                &'a valence::Valence,
            ) -> BoxFuture<'a, valence::Result<bool>>
            + Send
            + Sync
            + 'static,
    {
        let key = (table.to_string(), kind.map(str::to_string));
        let mut map = self
            .checks
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
    pub(crate) fn lookup(&self, table: &str, kind: &str) -> Option<NotificationAudienceCheck> {
        let map = self
            .checks
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.get(&(table.to_string(), Some(kind.to_string())))
            .or_else(|| map.get(&(table.to_string(), None)))
            .cloned()
    }
}

/// Register the audience check for notifications whose subject lives in `table`.
///
/// Pass `Some(kind)` to scope the check to one notification kind; `None`
/// covers every kind for that table. A kind-specific check wins over the
/// table-wide one. Registering the same key again replaces the earlier check
/// and logs a `warn`.
///
/// This fills [`NotificationAudiences::shared`], which applies when the host
/// installed no registry of its own or installed `shared()` itself.
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
    NotificationAudiences::shared().register(table, kind, check);
}

/// The check for `(table, kind)` from the registry installed on `v`, else the
/// shared default registry.
pub(crate) fn lookup_audience_check(
    v: &valence::Valence,
    table: &str,
    kind: &str,
) -> Option<NotificationAudienceCheck> {
    v.extension::<NotificationAudiences>()
        .unwrap_or_else(NotificationAudiences::shared)
        .lookup(table, kind)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{lookup_audience_check, register_notification_audience, NotificationAudiences};

    fn memory_valence(audiences: Option<Arc<NotificationAudiences>>) -> valence::Valence {
        let builder = valence::Valence::builder()
            .add_backend("default", Arc::new(valence::InMemoryBackend::new()));
        match audiences {
            Some(a) => a.install(builder),
            None => builder,
        }
        .build()
        .expect("build valence")
    }

    #[test]
    fn kind_specific_check_wins_and_table_wide_falls_back() {
        let audiences = NotificationAudiences::new();
        audiences.register("audience_unit_t", None, |_, _, _| {
            Box::pin(async { Ok(false) })
        });
        audiences.register("audience_unit_t", Some("special"), |_, _, _| {
            Box::pin(async { Ok(true) })
        });
        assert!(audiences.lookup("audience_unit_t", "special").is_some());
        assert!(audiences.lookup("audience_unit_t", "other").is_some());
        assert!(audiences
            .lookup("audience_unit_missing", "special")
            .is_none());
    }

    #[test]
    fn installed_registry_replaces_shared_and_shared_is_the_fallback() {
        register_notification_audience("audience_unit_shared", None, |_, _, _| {
            Box::pin(async { Ok(true) })
        });

        let without = memory_valence(None);
        assert!(lookup_audience_check(&without, "audience_unit_shared", "any").is_some());

        let installed = memory_valence(Some(Arc::new(NotificationAudiences::new())));
        assert!(lookup_audience_check(&installed, "audience_unit_shared", "any").is_none());

        let shared = memory_valence(Some(NotificationAudiences::shared()));
        assert!(lookup_audience_check(&shared, "audience_unit_shared", "any").is_some());
    }
}
