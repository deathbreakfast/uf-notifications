use valence::prelude::*;
use valence::privacy_policies::common::SYSTEM_ONLY;
use valence::privacy_policies::owner::OWNER_BY_USER_FIELD;

valence_schema! {
    NotificationSettings {
        repository: "https://github.com/unified-field-dev/uf-notifications",
        table: "notification_settings",
        version: "0.1.0",
        database: crate::embedded_surreal::DEFAULT_STORAGE,
        description: "One person's digest and quiet-hours settings; no row means instant delivery with quiet hours off",

        privacy: { gdpr_compliant: true },

        policies: {
            // Only the person reads and changes their own settings. System may
            // delete so account deletion can cascade.
            read:   { allow: [OWNER_BY_USER_FIELD] },
            create: { allow: [OWNER_BY_USER_FIELD] },
            update: { allow: [OWNER_BY_USER_FIELD] },
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
