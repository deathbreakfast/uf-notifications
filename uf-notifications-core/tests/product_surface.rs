//! Domain and mount-host surface contracts (privacy, schema, SECURITY, features).
//!
//! UI route/testid needles live with the product `uf-notifications` crate under
//! `unified-field-product`. This suite keeps Valence privacy + domain feature gates
//! runnable without the Orbital UI graph.

use std::fs;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn feature_assignment<'a>(toml: &'a str, name: &str) -> &'a str {
    let features = toml.split("[features]").nth(1).expect("[features] table");
    let marker = format!("{name} =");
    let start = features
        .find(&marker)
        .unwrap_or_else(|| panic!("missing feature {name}"));
    let rest = &features[start..];
    let mut end = rest.len();
    for (i, line) in rest.lines().enumerate() {
        if i == 0 {
            continue;
        }
        if line.starts_with(|c: char| c.is_ascii_alphabetic()) {
            end = rest.find(line).unwrap_or(rest.len());
            break;
        }
    }
    &rest[..end]
}

#[test]
fn notification_schema_create_system_or_auth_read_update_owner_happy_path() {
    let schema = fs::read_to_string(
        workspace_root().join("uf-notifications-core/schemas/notification_valence_schema.rs"),
    )
    .expect("notification_valence_schema.rs");
    assert!(
        schema.contains("create: { allow: [SYSTEM_ONLY, AUTHENTICATED] }"),
        "create must allow System jobs and session product fanout (no mid-request elevate)"
    );
    assert!(
        schema.contains("read:   { allow: [OWNER_BY_USER_FIELD] }")
            || schema.contains("read: { allow: [OWNER_BY_USER_FIELD] }"),
        "read must stay OWNER_BY_USER_FIELD"
    );
    assert!(
        schema.contains("update: { allow: [OWNER_BY_USER_FIELD] }"),
        "update must stay OWNER_BY_USER_FIELD for mark-read"
    );
    assert!(
        schema.contains("delete: { allow: [SYSTEM_ONLY] }"),
        "delete must stay SYSTEM_ONLY"
    );
}

#[test]
fn notification_schema_owner_create_drift_sad_path() {
    let schema = fs::read_to_string(
        workspace_root().join("uf-notifications-core/schemas/notification_valence_schema.rs"),
    )
    .expect("notification_valence_schema.rs");
    assert!(
        !schema.contains("create: { allow: [OWNER_BY_USER_FIELD] }"),
        "OWNER create would let any signed-in user spam notifications for themselves via session Valence"
    );
}

#[test]
fn production_features_omit_dev_tools_sad() {
    let rel = "uf-notifications-api/Cargo.toml";
    let toml = fs::read_to_string(workspace_root().join(rel)).expect(rel);
    assert!(
        toml.contains("dev-tools"),
        "{rel} must keep an explicit opt-in dev-tools feature"
    );
    for name in ["default", "ssr", "hydrate"] {
        let block = feature_assignment(&toml, name);
        assert!(
            !block.contains("dev-tools"),
            "{rel} feature `{name}` must not enable dev-tools:\n{block}"
        );
    }
}

#[test]
fn security_md_states_system_only_mint_happy_path() {
    let md = fs::read_to_string(workspace_root().join("SECURITY.md")).expect("SECURITY.md");
    for needle in [
        "SYSTEM_ONLY",
        "create_test_notification",
        "dev-tools",
        "OWNER_BY_USER_FIELD",
    ] {
        assert!(md.contains(needle), "SECURITY.md missing `{needle}`");
    }
}

#[test]
fn notifications_mount_host_contract_happy_path() {
    let host =
        fs::read_to_string(workspace_root().join("examples/notifications-mount-host/src/main.rs"))
            .expect("notifications-mount-host main.rs");
    for needle in [
        "const APP_ID: &str = \"notifications\"",
        "const ROUTE_PATH: &str = \"/notifications\"",
        "shell_notification_bell",
        "NotificationBell",
        "uf_notifications_core::send_notification",
        "/ws/notifications",
    ] {
        assert!(
            host.contains(needle),
            "notifications-mount-host missing contract `{needle}`"
        );
    }
}
