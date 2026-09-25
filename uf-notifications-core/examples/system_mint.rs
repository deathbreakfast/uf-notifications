//! System Valence mint via `send_notification`.
//!
//! Session Valence is denied by SYSTEM_ONLY create. This example builds an
//! in-memory router, seeds a user, and mints with `Actor::System`.
//!
//! ```bash
//! cargo run -p uf-notifications-core --example system_mint
//! ```

use std::sync::Arc;

use chrono::Utc;
use lepton_identity::generated::{User, UserStatus, UserUserType};
use uf_notifications_core::{send_notification, SendNotification};
use valence::{
    register_backend_logical_names, Actor, DatabaseBackend, DatabaseRouter, Model,
    RegisterBackendLogicalNamesOptions, SqliteBackend, Valence, SQLITE_ENGINE_ID,
};

#[tokio::main]
async fn main() {
    valence::deletion::register_noop_deletion_dispatcher_for_tests();
    valence::clear_for_test();
    if std::env::var_os("VALENCE_OWNERSHIP_UNIFIED_FETCH").is_none() {
        // SAFETY: example process only; OnceLock reads this before first ownership get.
        unsafe {
            std::env::set_var("VALENCE_OWNERSHIP_UNIFIED_FETCH", "0");
        }
    }

    let backend: Arc<dyn DatabaseBackend> = Arc::new(
        SqliteBackend::connect_memory()
            .await
            .expect("memory sqlite"),
    );
    let mut router = DatabaseRouter::new();
    register_backend_logical_names(
        &mut router,
        backend,
        &["default"],
        RegisterBackendLogicalNamesOptions::default(),
    );
    let system = Valence::builder()
        .database_router(Arc::new(router))
        .default_backend_key(valence::router_key("default", SQLITE_ENGINE_ID))
        .with_actor(Actor::System {
            operation: "notify_example_mint".into(),
        })
        .build()
        .expect("build valence");

    let now = Utc::now();
    let user = User::new(
        Some(UserUserType::Person),
        Some("example-hash".into()),
        Some(UserStatus::Active),
        None,
        None,
        Some(now),
        None,
        None,
        now,
        now,
    )
    .expect("user");
    User::upsert("example-user", user, &system, valence::use_!(r"When **notifications** needs to persist work, we **save User** so the next step in that feature can continue with the latest values. People and services allowed for **notifications** use this data for that workflow—not as a general export of unrelated personal fields."))
        .await
        .expect("seed user");

    let dto = send_notification(
        SendNotification {
            user_id: valence::RecordId::new("user", "example-user"),
            kind: "example".into(),
            title: "System mint".into(),
            message: "Created with Actor::System".into(),
            url: Some("/notifications".into()),
            data_json: None,
        },
        &system,
    )
    .await
    .expect("system mint");

    println!(
        "system_mint: OK — notification {} for example-user",
        dto.notification_id
    );
}
