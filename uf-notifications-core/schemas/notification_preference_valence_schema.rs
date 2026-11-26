use valence::prelude::*;
use valence::privacy_policies::common::SYSTEM_ONLY;
use valence::privacy_policies::owner::OWNER_BY_USER_FIELD;

valence_schema! {
    NotificationPreference {
        repository: "https://github.com/unified-field-dev/uf-notifications",
        table: "notification_preference",
        version: "0.1.0",
        database: crate::embedded_surreal::DEFAULT_STORAGE,
        description: "One person's channel choice for one notification kind; no row means the kind's defaults",

        privacy: { gdpr_compliant: true },

        policies: {
            // Only the person reads and changes their own choices. System may
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
            kind_id: {
                r#type: FieldType::String,
                required: true,
            },
            in_app: {
                r#type: FieldType::Boolean,
                required: true,
            },
            email: {
                r#type: FieldType::Boolean,
                required: true,
            },
            text: {
                r#type: FieldType::Boolean,
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
