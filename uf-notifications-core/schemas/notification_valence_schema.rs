use valence::prelude::*;
use valence::privacy_policies::common::SYSTEM_ONLY;
use valence::privacy_policies::owner::OWNER_BY_USER_FIELD;

use crate::create_policy::SCOPED_NOTIFICATION_CREATE;

valence_schema! {
    Notification {
        repository: "https://github.com/unified-field-dev/uf-notifications",
        table: "notification",
        version: "0.4.0",
        database: crate::embedded_surreal::DEFAULT_STORAGE,
        description: "User notification for inbox and bell",

        privacy: { gdpr_compliant: false },

        policies: {
            read:   { allow: [OWNER_BY_USER_FIELD] },
            // SYSTEM_ONLY for jobs/seeds. SCOPED_NOTIFICATION_CREATE lets product
            // side effects mint under the session actor without System elevation:
            // sender is the actor, the actor can read `subject`, and the recipient
            // is in the subject's registered audience (see `create_policy`).
            create: { allow: [SYSTEM_ONLY, SCOPED_NOTIFICATION_CREATE] },
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
            },
            sender: {
                r#type: FieldType::Record("user"),
                required: false,
            },
            subject: {
                r#type: FieldType::String,
                required: false,
            }
        ],

        connections: [
            user: {
                table: "user",
                on_delete: Cascade,
                model: "lepton_identity::generated::User",
            },
            // Deleting the sender clears the reference; the recipient keeps the row.
            sender: {
                table: "user",
                required: false,
                on_delete: SetNull,
                model: "lepton_identity::generated::User",
            },
        ],
    }
}
