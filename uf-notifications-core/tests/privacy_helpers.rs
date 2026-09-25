#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used)]
#![allow(dead_code)]

use std::sync::Arc;

use chrono::Utc;
use lepton_identity::generated::{User, UserStatus, UserUserType};
use valence::{
    register_backend_logical_names, Actor, DatabaseBackend, DatabaseRouter, Model,
    RegisterBackendLogicalNamesOptions, SqliteBackend, Valence, SQLITE_ENGINE_ID,
};

pub const TEST_USER_A: &str = "notify-test-user-a";
pub const TEST_USER_B: &str = "notify-test-user-b";

pub async fn setup_valence() -> Valence {
    valence::deletion::register_noop_deletion_dispatcher_for_tests();
    valence::clear_for_test();

    if std::env::var_os("VALENCE_OWNERSHIP_UNIFIED_FETCH").is_none() {
        // SAFETY: test harness only; OnceLock reads this before first ownership get.
        unsafe {
            std::env::set_var("VALENCE_OWNERSHIP_UNIFIED_FETCH", "0");
        }
    }

    let backend: Arc<dyn DatabaseBackend> = Arc::new(
        SqliteBackend::connect_memory()
            .await
            .expect("SqliteBackend::connect_memory"),
    );
    let mut router = DatabaseRouter::new();
    register_backend_logical_names(
        &mut router,
        backend,
        &["default"],
        RegisterBackendLogicalNamesOptions::default(),
    );

    Valence::builder()
        .database_router(Arc::new(router))
        .default_backend_key(valence::router_key("default", SQLITE_ENGINE_ID))
        .with_actor(Actor::System {
            operation: "notify_privacy_test".to_string(),
        })
        .build()
        .expect("build valence")
}

pub async fn seed_user(id: &str, valence: &Valence) {
    let now = Utc::now();
    let user = User::new(
        Some(UserUserType::Person),
        Some("test-password-hash".to_string()),
        Some(UserStatus::Active),
        None,
        None,
        Some(now),
        None,
        None,
        now,
        now,
    )
    .expect("build user");
    User::upsert(id, user, valence, valence::use_!(r"**Test:** Fixture **User** save for `tests` so the suite can arrange and assert persistence behavior. CI and developers running the suite only.")).await.expect("upsert user");
}

pub async fn setup_shared_db() -> Valence {
    let v = setup_valence().await;
    seed_user(TEST_USER_A, &v).await;
    seed_user(TEST_USER_B, &v).await;
    v
}

pub fn as_user(base: &Valence, user_id: &str) -> Valence {
    base.with_actor(Actor::User {
        user_id: user_id.to_string(),
    })
}

pub fn as_system(base: &Valence) -> Valence {
    base.with_actor(Actor::System {
        operation: "notify_privacy_test".to_string(),
    })
}
