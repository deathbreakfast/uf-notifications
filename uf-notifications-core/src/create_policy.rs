//! Scoped Notification create policy.
//!
//! [`SCOPED_NOTIFICATION_CREATE`] lets a non-System actor create a notification
//! under its own privacy, with no System elevation:
//!
//! - **Row with `subject`** (all must hold):
//!   1. `sender` is set and names the actor (`table:` prefix ignored). A
//!      service user passes only when the sender is that service user.
//!   2. The actor can read `subject` through its own [`Valence`].
//!   3. The recipient (`user`) is in the subject's audience, answered by the
//!      check registered with [`crate::register_notification_audience`]. With
//!      no check registered, only `recipient == actor` passes.
//! - **Row without `subject`** (legacy, removed in phase B): allowed for any
//!   authenticated actor, with a `warn` naming the kind. If `sender` is set it
//!   must still name the actor.
//!
//! System actors are covered by `SYSTEM_ONLY` in the schema and pass here too.

use std::any::Any;

use async_trait::async_trait;
use valence::{
    Actor, ActorContext, Error, PolicyEvaluator, PrivacyEvaluator, PrivacyOperation, QueryCore,
    RecordId, Result, SchemaRegistry, Valence,
};

use crate::audience::lookup_audience_check;

/// Scoped create gate for `notification` rows; see the [module docs](self).
#[derive(Debug, Clone, Copy)]
pub struct ScopedNotificationCreate;

/// Shared [`ScopedNotificationCreate`] for schema `allow: [SCOPED_NOTIFICATION_CREATE]`.
pub const SCOPED_NOTIFICATION_CREATE: ScopedNotificationCreate = ScopedNotificationCreate;

#[async_trait]
impl PolicyEvaluator for ScopedNotificationCreate {
    fn name(&self) -> &'static str {
        "uf_notifications::SCOPED_NOTIFICATION_CREATE"
    }

    fn description(&self) -> Option<&'static str> {
        Some(
            "Sender is the actor, the actor can read the subject, and the recipient is in the subject's audience",
        )
    }

    async fn evaluate(
        &self,
        _op: PrivacyOperation,
        record: &serde_json::Value,
        actor: &dyn ActorContext,
        v: &Valence,
    ) -> Result<bool> {
        let actor: Actor = serde_json::from_value(actor.actor_json().clone())
            .map_err(|e| Error::Internal(format!("invalid actor context: {e}")))?;
        if actor.is_system() {
            return Ok(true);
        }
        let Some(actor_id) = actor_identity(&actor) else {
            return Ok(false);
        };

        let kind = record.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        let sender = record
            .get("sender")
            .filter(|s| !s.is_null())
            .map(record_ref_id);
        let Some(subject) = record
            .get("subject")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
        else {
            // A sender, when present, must still be the actor so legacy rows
            // can't name someone else as the sender.
            if sender.is_some_and(|s| s.as_deref() != Some(actor_id)) {
                return Ok(false);
            }
            tracing::warn!(
                operation = "create",
                policy = "scoped_notification_create",
                kind,
                "notification created without a subject (legacy path, removed in phase B)"
            );
            return Ok(true);
        };

        // 1. The sender is the actor.
        if sender.flatten().as_deref() != Some(actor_id) {
            return Ok(false);
        }

        // 2. The actor can read the subject under its own privacy.
        if !actor_can_read_subject(subject, v).await? {
            return Ok(false);
        }

        // 3. The recipient is in the subject's audience.
        let Some(recipient) = record.get("user").and_then(record_ref_id) else {
            return Ok(false);
        };
        let Some(table) = RecordId::parse(subject).map(|r| r.table().to_string()) else {
            return Ok(false);
        };
        match lookup_audience_check(&table, kind) {
            Some(check) => check(subject, &recipient, v).await,
            None => Ok(recipient == actor_id),
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The id a `sender` must carry for this actor: the user id, or the service name.
fn actor_identity(actor: &Actor) -> Option<&str> {
    actor.user_id().or_else(|| actor.service_name())
}

/// Id part of a record reference stored as `table:id`, a bare id, or `{ id }`.
fn record_ref_id(value: &serde_json::Value) -> Option<String> {
    if let Ok(rid) = serde_json::from_value::<RecordId>(value.clone()) {
        return Some(rid.id().to_string());
    }
    let raw = match value {
        serde_json::Value::String(s) => s.as_str(),
        serde_json::Value::Object(map) => map.get("id")?.as_str()?,
        _ => return None,
    };
    let id = raw.split_once(':').map_or(raw, |(_, id)| id);
    (!id.is_empty()).then(|| id.to_string())
}

/// Load `subject` with the caller's Valence and run its Read policies.
async fn actor_can_read_subject(subject: &str, v: &Valence) -> Result<bool> {
    let Some(subject_id) = RecordId::parse(subject) else {
        return Ok(false);
    };
    let Some(schema) = SchemaRegistry::lookup(subject_id.table()) else {
        return Ok(false);
    };
    let raw = match QueryCore::get_record_json(
        subject_id.table(),
        subject_id.id(),
        v,
        valence::use_!(
            r"**Notifications:** Load the **subject record** a notification is about, as the sender, to confirm the sender can see it before the notification is created."
        ),
    )
    .await
    {
        Ok(Some(raw)) => raw,
        Ok(None) | Err(Error::Privacy(_)) => return Ok(false),
        Err(e) => return Err(e),
    };
    match PrivacyEvaluator::check_entity_access(schema, PrivacyOperation::Read, &raw, v).await {
        Ok(()) => Ok(true),
        Err(Error::Privacy(_)) => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::record_ref_id;
    use serde_json::json;

    #[test]
    fn record_ref_id_strips_table_prefix_happy() {
        assert_eq!(
            record_ref_id(&json!("user:alice")).as_deref(),
            Some("alice")
        );
        assert_eq!(record_ref_id(&json!("alice")).as_deref(), Some("alice"));
        assert_eq!(
            record_ref_id(&json!({ "id": "user:alice" })).as_deref(),
            Some("alice")
        );
    }

    #[test]
    fn record_ref_id_rejects_empty_and_non_refs_sad() {
        assert_eq!(record_ref_id(&json!("")), None);
        assert_eq!(record_ref_id(&json!(42)), None);
        assert_eq!(record_ref_id(&serde_json::Value::Null), None);
    }
}
