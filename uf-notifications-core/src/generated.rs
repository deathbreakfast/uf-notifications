#![allow(
    dead_code,
    unused_imports,
    missing_docs,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::restriction
)]
//! `Notification` Valence model generated from `schemas/notification_valence_schema.rs` by
//! `build.rs` (via `valence_codegen`).
//!
//! Contents are produced at build time and intentionally left undocumented here; see the
//! schema file for field-level semantics.

use valence::privacy_policies::common::{AUTHENTICATED, PUBLIC_READ, SYSTEM_ONLY};
use valence::privacy_policies::owner::{OWNER_BY_ID, OWNER_BY_USER_FIELD};

include!(concat!(env!("OUT_DIR"), "/generated_models.rs"));
