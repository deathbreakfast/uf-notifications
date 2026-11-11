//! `notifications.send`: a Catalyst action that notifies people.
//!
//! Enabled by the `catalyst` feature. Register it once at host boot, next to the
//! core kinds, with [`register_notifications_send`].
//!
//! Params: `to` (people, teams allowed), `text` (template) and `link` (template,
//! optional). Teams expand through the host's
//! [`TeamMembers`](::catalyst::people::TeamMembers) hook. The action writes
//! notifications people read, so [`ActionKind::writes_visible`] is true and a
//! rule may run it as a bot. It outputs `sent`, the number of people notified.
//!
//! # Who sends
//!
//! Catalyst gives an action no Valence handle, so the host supplies a
//! [`NotifyActor`]. It returns a handle for the owner, or for the bot's service
//! user when [`ExecContext::run_as`] is a bot. The action never builds an actor
//! itself and never elevates to System; [`send_notification`] records the
//! handle's actor as `sender`.
//!
//! # Who is skipped
//!
//! Each notification is created under the sending actor's own privacy. The
//! fire's subject (`kind:id`) becomes the notification `subject`, so the actor
//! must be able to read it and the recipient must pass the audience check the
//! host registered with
//! [`register_notification_audience`](uf_notifications_core::register_notification_audience).
//! A recipient Valence refuses is skipped and counted: the outcome status is
//! `partial` and the outputs carry `skipped`. Any other failure fails the
//! action with a `construct_failed` or `persist_failed` reason, so a storage
//! fault is never reported as a skip. A failed action does not roll back the
//! notifications already sent, so a retry notifies those recipients again.

use std::future::Future;
use std::pin::Pin;

use ::catalyst::registry::{
    register_action, ActionKind, ActionOutcome, CatalystRegistries, ContextField, ContextSchema,
    ContextType, ExecContext, KindMeta, ParamField, ParamFieldKind, ParamSchema, RuleRunRef,
    SimulatedAction, SubjectRef,
};
use ::catalyst::CatalystError;
use serde_json::{json, Value};
use valence::{RecordId, Valence};

use uf_notifications_core::{
    sanitize_notification_url, send_notification, SendNotification, SendNotificationError,
};

/// Registry id of [`NotifySend`].
pub const NOTIFICATIONS_SEND: &str = "notifications.send";

/// Notification `kind` the action writes.
const NOTIFY_KIND: &str = "catalyst";
/// Notification title the action writes.
const NOTIFY_TITLE: &str = "Catalyst rule";

/// Host hook that returns the Valence handle a fire's notifications are sent
/// as.
///
/// Return a handle scoped to the rule owner, or to the bot's service user when
/// `ctx.run_as()` is [`RunAs::Bot`](::catalyst::registry::RunAs::Bot). Do not
/// return a System handle to get around privacy.
pub trait NotifyActor: Send + Sync {
    /// Handle for the actor this fire sends as.
    ///
    /// # Errors
    ///
    /// [`CatalystError`] when the actor cannot be resolved.
    fn valence_for<'a>(
        &'a self,
        ctx: &'a ExecContext,
    ) -> Pin<Box<dyn Future<Output = Result<Valence, CatalystError>> + Send + 'a>>;
}

/// Action: notify the people in `to`.
///
/// `simulate` expands the same recipients and reports `would_notify`; it
/// writes nothing and never asks the [`NotifyActor`] for a handle.
pub struct NotifySend<A> {
    actor: A,
}

impl<A: NotifyActor> NotifySend<A> {
    /// Build the action around the host's [`NotifyActor`].
    #[must_use]
    pub const fn new(actor: A) -> Self {
        Self { actor }
    }
}

/// Register [`NotifySend`] into `regs`.
///
/// # Errors
///
/// [`CatalystError::DuplicateKindId`] when `notifications.send` is already
/// registered.
pub fn register_notifications_send(
    regs: &mut CatalystRegistries,
    actor: impl NotifyActor + 'static,
) -> Result<(), CatalystError> {
    register_action(regs, NotifySend::new(actor))
}

fn text_param<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn invalid(reason: &'static str) -> CatalystError {
    CatalystError::kind_failed("notifications.send", NOTIFICATIONS_SEND, reason)
}

impl<A: NotifyActor> ActionKind for NotifySend<A> {
    fn id(&self) -> &'static str {
        NOTIFICATIONS_SEND
    }

    fn meta(&self) -> KindMeta {
        KindMeta::new("Notifications", "Send notification")
    }

    fn param_schema(&self) -> ParamSchema {
        ParamSchema::empty()
            .with_field(ParamField::required(
                "to",
                "To",
                ParamFieldKind::People {
                    single: false,
                    allow_team: true,
                },
            ))
            .and_then(|s| {
                s.with_field(ParamField::required(
                    "text",
                    "Message",
                    ParamFieldKind::Template,
                ))
            })
            .and_then(|s| {
                s.with_field(ParamField::optional(
                    "link",
                    "Link",
                    ParamFieldKind::Template,
                ))
            })
            .unwrap_or_default()
    }

    fn validate_params(&self, params: &Value) -> Result<(), CatalystError> {
        if text_param(params, "text").is_none() {
            return Err(invalid("missing_text"));
        }
        // A template link is checked once it resolves, at send time.
        if let Some(link) = text_param(params, "link").filter(|l| !l.contains("{{")) {
            if sanitize_notification_url(Some(link.to_string())).is_none() {
                return Err(invalid("unsafe_link"));
            }
        }
        Ok(())
    }

    fn requires_step_up_to_go_live(&self) -> bool {
        false
    }

    fn requires_matching_backtest(&self) -> bool {
        false
    }

    fn output_schema(&self) -> ContextSchema {
        ContextSchema::new(vec![
            ContextField::new("sent", "People notified", ContextType::Number),
            ContextField::new("skipped", "People skipped", ContextType::Number),
        ])
    }

    fn writes_visible(&self) -> bool {
        true
    }

    fn execute<'a>(
        &'a self,
        ctx: &'a ExecContext,
        params: &'a Value,
        subject: &'a SubjectRef,
        _run: &'a RuleRunRef,
    ) -> Pin<Box<dyn Future<Output = Result<ActionOutcome, CatalystError>> + Send + 'a>> {
        Box::pin(async move {
            let text = text_param(params, "text").ok_or_else(|| invalid("missing_text"))?;
            let people = params.get("to").cloned().unwrap_or(Value::Null);
            let recipients = ctx.expand_people(&people).await?;
            let valence = self.actor.valence_for(ctx).await?;
            let url = text_param(params, "link").map(str::to_string);
            let subject = format!("{}:{}", subject.kind, subject.id);

            let (mut sent, mut skipped) = (0_u64, 0_u64);
            for recipient in recipients {
                let result = send_notification(
                    SendNotification {
                        user_id: RecordId::new("user", recipient),
                        kind: NOTIFY_KIND.into(),
                        title: NOTIFY_TITLE.into(),
                        message: text.to_string(),
                        url: url.clone(),
                        subject: Some(subject.clone()),
                        ..Default::default()
                    },
                    &valence,
                )
                .await;
                match result {
                    Ok(_) => sent += 1,
                    Err(SendNotificationError::Persist {
                        source: valence::Error::Privacy(_),
                        ..
                    }) => skipped += 1,
                    Err(err) => {
                        let reason = match err {
                            SendNotificationError::Construct(_) => "construct_failed",
                            _ => "persist_failed",
                        };
                        tracing::warn!(
                            operation = "notifications.send",
                            reason,
                            sent,
                            skipped,
                            "notification send failed; stopping the action"
                        );
                        return Err(invalid(reason));
                    }
                }
            }
            let status = if skipped == 0 { "ok" } else { "partial" };
            Ok(ActionOutcome::new(status).with_outputs(json!({"sent": sent, "skipped": skipped})))
        })
    }

    fn simulate<'a>(
        &'a self,
        ctx: &'a ExecContext,
        params: &'a Value,
        _subject: &'a SubjectRef,
        _at: chrono::DateTime<chrono::Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<SimulatedAction, CatalystError>> + Send + 'a>> {
        Box::pin(async move {
            let people = params.get("to").cloned().unwrap_or(Value::Null);
            let recipients = ctx.expand_people(&people).await?;
            Ok(SimulatedAction::new(format!(
                "would_notify:{}",
                recipients.join(",")
            )))
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::sync::Arc;

    use ::catalyst::people::TeamMembers;

    use super::*;

    struct NoActor;

    impl NotifyActor for NoActor {
        fn valence_for<'a>(
            &'a self,
            _ctx: &'a ExecContext,
        ) -> Pin<Box<dyn Future<Output = Result<Valence, CatalystError>> + Send + 'a>> {
            Box::pin(async { Err(CatalystError::internal("no actor in this test")) })
        }
    }

    struct Teams;

    impl TeamMembers for Teams {
        fn members<'a>(
            &'a self,
            _ctx: &'a ExecContext,
            team_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, CatalystError>> + Send + 'a>> {
            let out = match team_id {
                "t-sec" => Ok(vec!["ana".to_string(), "bo".to_string()]),
                _ => Err(CatalystError::internal("teams.members")),
            };
            Box::pin(async move { out })
        }
    }

    fn action() -> NotifySend<NoActor> {
        NotifySend::new(NoActor)
    }

    fn simulate(ctx: &ExecContext, params: &Value) -> Result<SimulatedAction, CatalystError> {
        let subject = SubjectRef::new("task", "t1");
        let act = action();
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(act.simulate(ctx, params, &subject, chrono::Utc::now()))
    }

    #[test]
    fn registers_as_visible_notify_only_action_happy() {
        let mut regs = CatalystRegistries::new();
        register_notifications_send(&mut regs, NoActor).expect("register");
        let kind = regs.get_action(NOTIFICATIONS_SEND).expect("registered");
        assert!(kind.writes_visible(), "Run as bot needs visible writes");
        assert!(!kind.requires_matching_backtest());
        assert!(kind.output_schema().field("sent").is_some());
        assert!(kind.output_schema().field("skipped").is_some());
        let keys: Vec<_> = kind
            .param_schema()
            .fields()
            .iter()
            .map(|f| f.key.clone())
            .collect();
        assert_eq!(keys, ["to", "text", "link"]);
    }

    #[test]
    fn registering_twice_is_a_duplicate_sad() {
        let mut regs = CatalystRegistries::new();
        register_notifications_send(&mut regs, NoActor).expect("first");
        let err = register_notifications_send(&mut regs, NoActor).expect_err("second");
        assert!(matches!(err, CatalystError::DuplicateKindId { .. }));
    }

    #[test]
    fn validate_params_accepts_text_and_safe_or_template_link_happy() {
        let a = action();
        assert!(a.validate_params(&json!({"text": "hi"})).is_ok());
        assert!(a
            .validate_params(&json!({"text": "hi", "link": "/tasks/1"}))
            .is_ok());
        assert!(a
            .validate_params(&json!({"text": "hi", "link": "/tasks/{{trigger.id}}"}))
            .is_ok());
    }

    #[test]
    fn validate_params_rejects_empty_text_and_unsafe_link_sad() {
        let a = action();
        assert!(a.validate_params(&json!({"text": "  "})).is_err());
        assert!(a.validate_params(&json!({})).is_err());
        assert!(a
            .validate_params(&json!({"text": "hi", "link": "//evil.example"}))
            .is_err());
    }

    #[test]
    fn simulate_lists_person_and_team_members_deduped_happy() {
        let ctx = ExecContext::new().with_team_members(Some(Arc::new(Teams)));
        let params =
            json!({"to": [{"user": "ana"}, {"team": "t-sec"}, {"user": "cy"}], "text": "x"});
        let out = simulate(&ctx, &params).expect("simulate");
        assert_eq!(out.status, "would_notify:ana,bo,cy");
    }

    #[test]
    fn simulate_fails_on_team_without_hook_or_with_hook_error_sad() {
        let params = json!({"to": [{"team": "t-sec"}], "text": "x"});
        assert!(simulate(&ExecContext::new(), &params).is_err());
        let ctx = ExecContext::new().with_team_members(Some(Arc::new(Teams)));
        let down = json!({"to": [{"team": "t-down"}], "text": "x"});
        assert!(simulate(&ctx, &down).is_err());
    }
}
