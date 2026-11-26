//! Retention for notification rows.
//!
//! The Chronon script `notifications.retention` runs daily (default job
//! `notifications-retention`, cron `30 3 * * *`). The host registers default
//! jobs with a System Valence, so the job starts as System and can delete
//! rows (`notification` delete is `SYSTEM_ONLY`); nothing elevates inside a
//! request. It deletes read notifications [`READ_RETENTION_DAYS`] after
//! `read_at` and unread ones [`UNREAD_RETENTION_DAYS`] after `created_at`.

use chrono::{DateTime, Duration, Utc};
use valence::{DateTimePredicate, Model, Valence};

use crate::Notification;

/// Days a read notification is kept after it was read.
pub const READ_RETENTION_DAYS: i64 = 90;
/// Days an unread notification is kept after it was created.
pub const UNREAD_RETENTION_DAYS: i64 = 365;
/// Rows loaded per delete batch.
const BATCH: u32 = 500;

async fn delete_rows(valence: &Valence, rows: Vec<Notification>) -> valence::Result<usize> {
    let mut deleted = 0;
    for row in rows {
        let Some(id) = row.id() else { continue };
        // Immediate delete, not a queued run: these rows are already past their
        // retention window, have no rows that depend on them, and the job
        // deletes in bounded batches.
        Notification::delete_now(id.id(), valence, valence::use_!(r"When the **notification retention** job runs each day, we **delete Notification** rows read more than 90 days ago or unread for more than a year, so old notifications don't pile up. Only the retention job deletes them this way.")).await?;
        deleted += 1;
    }
    Ok(deleted)
}

/// Delete every notification past retention at `now`. Returns how many.
///
/// Run as System from the host's Chronon job.
///
/// # Errors
///
/// Returns the Valence error when a query or delete fails; rows deleted
/// before it stay deleted.
pub async fn retention_with_valence(
    valence: &Valence,
    now: DateTime<Utc>,
) -> valence::Result<usize> {
    let read_cutoff = now - Duration::days(READ_RETENTION_DAYS);
    let unread_cutoff = now - Duration::days(UNREAD_RETENTION_DAYS);
    let mut deleted = 0;
    loop {
        let rows: Vec<Notification> = Notification::query(valence, valence::use_!(r"When the **notification retention** job runs each day, we **list Notification** rows read more than 90 days ago so they can be deleted. Only the retention job reads them this way."))
            .where_read_at(DateTimePredicate::Before(read_cutoff))
            .limit(BATCH)
            .await?;
        let n = rows.len();
        deleted += delete_rows(valence, rows).await?;
        if n < BATCH as usize {
            break;
        }
    }
    loop {
        let rows: Vec<Notification> = Notification::query(valence, valence::use_!(r"When the **notification retention** job runs each day, we **list Notification** rows left unread for more than a year so they can be deleted. Only the retention job reads them this way."))
            .where_read_at_is_none()
            .where_created_at(DateTimePredicate::Before(unread_cutoff))
            .limit(BATCH)
            .await?;
        let n = rows.len();
        deleted += delete_rows(valence, rows).await?;
        if n < BATCH as usize {
            break;
        }
    }
    Ok(deleted)
}

mod job {
    #![allow(dead_code, missing_docs, clippy::pedantic, clippy::nursery)]

    #[chronon_coordinator_macros::script(
        name = "notifications.retention",
        default_job(job = "notifications-retention", cron = "30 3 * * *")
    )]
    pub(crate) async fn notifications_retention(
        ctx: Box<dyn chronon_core::ScriptContext>,
    ) -> anyhow::Result<()> {
        let v = chronon_valence_identity::valence_from_context(&*ctx)?;
        let deleted = super::retention_with_valence(&v, chrono::Utc::now()).await?;
        tracing::info!(operation = "retention", deleted);
        Ok(())
    }
}
