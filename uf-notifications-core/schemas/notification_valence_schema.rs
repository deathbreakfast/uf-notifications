use valence::prelude::*;
use valence::privacy_policies::common::SYSTEM_ONLY;
use valence::privacy_policies::owner::OWNER_BY_USER_FIELD;

valence_schema! {
    Notification {
        repository: "https://github.com/unified-field-dev/uf-notifications",
        table: "notification",
        version: "0.2.0",
        database: crate::embedded_surreal::DEFAULT_STORAGE,
        description: "User notification for inbox and bell",

        privacy: { gdpr_compliant: false },

        policies: {
            read:   { allow: [OWNER_BY_USER_FIELD] },
            // System only: a signed-in actor must not write into another user's
            // inbox. Products mint from jobs (e.g. Boson tasks) that start as System.
            create: { allow: [SYSTEM_ONLY] },
            update: { allow: [OWNER_BY_USER_FIELD] },
            delete: { allow: [SYSTEM_ONLY] },
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
            kind: {
                r#type: FieldType::String,
                required: true,
            },
            title: {
                r#type: FieldType::String,
                required: true,
            },
            message: {
                r#type: FieldType::String,
                required: true,
            },
            url: {
                r#type: FieldType::String,
                required: false,
            },
            data_json: {
                r#type: FieldType::String,
                required: false,
            },
            read_at: {
                r#type: FieldType::Datetime,
                required: false,
            },
            created_at: {
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
