//! Runtime schema registration for notifications.
//!
//! Model schemas are registered by codegen (`OUT_DIR/generated_models.rs`). Including
//! `*_valence_schema.rs` here would submit a second [`valence::SchemaMetadataInit`]
//! and panic in [`valence::SchemaRegistry`].
//!
//! Schemas in `schemas/`: `Notification`, `NotificationPreference` (one row per kind a
//! person changed) and `NotificationSettings` (one row per person). The two
//! preference tables are owner-only and cascade-delete with the user.
