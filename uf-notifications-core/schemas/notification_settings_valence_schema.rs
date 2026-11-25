use valence::prelude::*;
use valence::privacy_policies::common::SYSTEM_ONLY;
use valence::privacy_policies::owner::OWNER_BY_USER_FIELD;

use crate::delivery_policy::NOTIFICATION_DELIVERY;

valence_schema! {
    NotificationSettings {
        repository: "https://github.com/unified-field-dev/uf-notifications",
        table: "notification_settings",
        version: "0.2.0",
        database: crate::embedded_surreal::DEFAULT_STORAGE,
        description: "One person's digest and quiet-hours settings; no row means instant delivery with quiet hours off",

        privacy: { gdpr_compliant: true },

        policies: {
            // Only the person changes their own settings. The delivery actor
            // reads them to time email and text; the digest job (System) reads
            // them and records `last_digest_at`. System may delete so account
            // deletion can cascade.
            read:   { allow: [OWNER_BY_USER_FIELD, SYSTEM_ONLY, NOTIFICATION_DELIVERY] },
            create: { allow: [OWNER_BY_USER_FIELD] },
            update: { allow: [OWNER_BY_USER_FIELD, SYSTEM_ONLY] },
            delete: { allow: [SYSTEM_ONLY, OWNER_BY_USER_FIELD] },
        },

        fields: [
            id: {
                r#type: FieldType::String,
                primary_key: true,
                required: true,
            },
            user: {
                r#type: FieldType::Record("user"),
                required: true,
            },
            digest: {
                r#type: FieldType::Enum(&["instant", "daily", "weekly"]),
                required: true,
            },
            quiet_enabled: {
                r#type: FieldType::Boolean,
                required: true,
            },
            // Local wall-clock times, "HH:MM".
            quiet_start: {
                r#type: FieldType::String,
                required: true,
            },
            quiet_end: {
                r#type: FieldType::String,
                required: true,
            },
            time_zone: {
                r#type: FieldType::String,
                required: true,
            },
            updated_at: {
                r#type: FieldType::Datetime,
                required: true,
            },
            // When the last digest email went out.
            last_digest_at: {
                r#type: FieldType::Datetime,
                required: false,
            }
        ],

        connections: [
            user: {
                table: "user",
                on_delete: Cascade,
                model: "lepton_identity::generated::User",
            },
        ],
    }
}
