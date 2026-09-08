//! Runtime schema registration for notifications.
//!
//! Model schemas are registered by codegen (`OUT_DIR/generated_models.rs`). Including
//! `*_valence_schema.rs` here would submit a second [`valence::SchemaMetadataInit`]
//! and panic in [`valence::SchemaRegistry`].
