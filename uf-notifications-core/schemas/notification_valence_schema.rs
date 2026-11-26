use valence::prelude::*;
use valence::privacy_policies::common::SYSTEM_ONLY;
use valence::privacy_policies::owner::OWNER_BY_USER_FIELD;

use crate::create_policy::SCOPED_NOTIFICATION_CREATE;
use crate::delivery_policy::NOTIFICATION_DELIVERY;

valence_schema! {
    Notification {
        repository: "https://github.com/unified-field-dev/uf-notifications",
        table: "notification",
        version: "0.6.0",
        database: crate::embedded_surreal::DEFAULT_STORAGE,
        description: "User notification for inbox and bell",

        privacy: { gdpr_compliant: false },

        policies: {
            // SYSTEM_ONLY lets the digest and retention jobs (started as System
            // by the host) read their rows. NOTIFICATION_DELIVERY is the
            // `svc:notifications` service actor that delivers email and text.
            read:   { allow: [OWNER_BY_USER_FIELD, SYSTEM_ONLY, NOTIFICATION_DELIVERY] },
            // SYSTEM_ONLY for jobs/seeds. SCOPED_NOTIFICATION_CREATE lets product
            // side effects mint under the session actor without System elevation:
            // sender is the actor, the actor can read `subject`, and the recipient
            // is in the subject's registered audience (see `create_policy`).
            // Without a subject, only a note to yourself passes. `sender` stays
            // optional in the schema (System rows have none); the policy
            // requires it for every non-System create.
            create: { allow: [SYSTEM_ONLY, SCOPED_NOTIFICATION_CREATE] },
            // The delivery actor records `email_sent_at`, `text_sent_at` and
            // `delivery_error`.
            update: { allow: [OWNER_BY_USER_FIELD, NOTIFICATION_DELIVERY] },
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
            },
            email_sent_at: {
                r#type: FieldType::Datetime,
                required: false,
            },
            text_sent_at: {
                r#type: FieldType::Datetime,
                required: false,
            },
            // Last email or text failure, cleared on the next success.
            delivery_error: {
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
