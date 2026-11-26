use valence::prelude::*;
use valence::privacy_policies::common::SYSTEM_ONLY;
use valence::privacy_policies::owner::OWNER_BY_USER_FIELD;

use crate::delivery_policy::NOTIFICATION_DELIVERY;

valence_schema! {
    NotificationContact {
        repository: "https://github.com/unified-field-dev/uf-notifications",
        table: "notification_contact",
        version: "0.1.0",
        database: crate::embedded_surreal::DEFAULT_STORAGE,
        description: "One person's verified phone number for text notifications, and a pending verification code",

        privacy: { gdpr_compliant: true },

        policies: {
            // Only the person adds or changes their number. The delivery actor
            // and the digest job (System) read the verified number to send.
            // System may delete so account deletion can cascade.
            read:   { allow: [OWNER_BY_USER_FIELD, SYSTEM_ONLY, NOTIFICATION_DELIVERY] },
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
            // Verified number, E.164.
            phone: {
                r#type: FieldType::String,
                required: false,
            },
            phone_verified_at: {
                r#type: FieldType::Datetime,
                required: false,
            },
            // Number waiting for its code.
            pending_phone: {
                r#type: FieldType::String,
                required: false,
            },
            // Hex SHA-256 of salt + code; the code itself is never stored.
            code_hash: {
                r#type: FieldType::String,
                required: false,
            },
            code_salt: {
                r#type: FieldType::String,
                required: false,
            },
            code_expires_at: {
                r#type: FieldType::Datetime,
                required: false,
            },
            code_tries: {
                r#type: FieldType::Integer,
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
