//! Privacy rule for the notification delivery service actor.
//!
//! Email and text go out from background work, not from the request that
//! created the notification. That work runs as one narrow service actor,
//! [`delivery_actor`] (`Actor::ServiceUser { service_name: "svc:notifications" }`),
//! never as System and never as the sender. [`NOTIFICATION_DELIVERY`] admits
//! exactly that actor, and the schemas list it only where delivery needs it:
//!
//! - read on `notification`, `notification_preference`, `notification_settings`
//!   and `notification_contact`, to decide what to send and where;
//! - update on `notification`, to record `email_sent_at`, `text_sent_at` and
//!   `delivery_error`.
//!
//! It passes no other rule: a `ServiceUser` never matches owner rules or
//! `SYSTEM_ONLY`, and any other service name fails here.

use std::any::Any;

use async_trait::async_trait;
use valence::{Actor, ActorContext, Error, PolicyEvaluator, PrivacyOperation, Result, Valence};

/// Service name of the actor that delivers email and text.
pub const DELIVERY_SERVICE_NAME: &str = "svc:notifications";

/// The actor delivery tasks run as.
pub fn delivery_actor() -> Actor {
    Actor::ServiceUser {
        service_name: DELIVERY_SERVICE_NAME.to_string(),
    }
}

/// Admits only the [`delivery_actor`]; see the [module docs](self).
#[derive(Debug, Clone, Copy)]
pub struct NotificationDelivery;

/// Shared [`NotificationDelivery`] for schema `allow: [NOTIFICATION_DELIVERY]`.
pub const NOTIFICATION_DELIVERY: NotificationDelivery = NotificationDelivery;

#[async_trait]
impl PolicyEvaluator for NotificationDelivery {
    fn name(&self) -> &'static str {
        "uf_notifications::NOTIFICATION_DELIVERY"
    }

    fn description(&self) -> Option<&'static str> {
        Some("The svc:notifications service actor that delivers email and text")
    }

    async fn evaluate(
        &self,
        _op: PrivacyOperation,
        _record: &serde_json::Value,
        actor: &dyn ActorContext,
        _v: &Valence,
    ) -> Result<bool> {
        let actor: Actor = serde_json::from_value(actor.actor_json().clone())
            .map_err(|e| Error::Internal(format!("invalid actor context: {e}")))?;
        Ok(actor.service_name() == Some(DELIVERY_SERVICE_NAME))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::{delivery_actor, DELIVERY_SERVICE_NAME};

    #[test]
    fn delivery_actor_is_the_named_service_user() {
        let actor = delivery_actor();
        assert_eq!(actor.service_name(), Some(DELIVERY_SERVICE_NAME));
        assert!(!actor.is_system());
        assert_eq!(
            serde_json::to_value(&actor).expect("serialize"),
            serde_json::json!({ "ServiceUser": { "service_name": "svc:notifications" } })
        );
    }
}
