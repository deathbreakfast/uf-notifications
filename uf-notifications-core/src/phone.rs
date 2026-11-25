//! Phone number verification for text notifications.
//!
//! [`start_phone_verification`] texts a 6-digit code to a number through the
//! host's Text channel and keeps only a salted SHA-256 of it, for 10 minutes
//! and at most [`MAX_CODE_TRIES`] tries. [`confirm_phone_verification`]
//! checks the code and, when it matches, stores the number as verified on the
//! person's [`NotificationContact`](crate::NotificationContact) row. That row
//! is owner-only and deleted with the account. Both run as the signed-in
//! person; nothing here elevates.

use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use sha2::{Digest as _, Sha256};
use valence::{Model, RecordId, Valence};

use crate::channels::{
    ChannelError, ChannelKind, Contact, NotificationChannels, OutgoingNotification,
    PHONE_CODE_KIND_ID,
};
use crate::generated::NotificationContact;

/// How long a code stays valid.
pub const CODE_TTL_MINUTES: i64 = 10;
/// Wrong codes allowed before the code is refused; start again for a new one.
pub const MAX_CODE_TRIES: i64 = 5;
/// Shortest gap between two codes sent to the same person.
pub const RESEND_AFTER_SECONDS: i64 = 60;

/// Why phone verification failed.
#[derive(Debug)]
pub enum PhoneVerificationError {
    /// The number is not E.164 (`+` and 8 to 15 digits).
    InvalidNumber,
    /// The host installed no Text channel.
    TextNotConfigured,
    /// A code went out less than [`RESEND_AFTER_SECONDS`] ago.
    TooSoon,
    /// There is no code waiting; start verification first.
    NoPendingCode,
    /// The code is older than [`CODE_TTL_MINUTES`].
    Expired,
    /// The code does not match.
    WrongCode,
    /// [`MAX_CODE_TRIES`] wrong codes were entered.
    TooManyTries,
    /// The Text channel could not send the code.
    Send(ChannelError),
    /// Valence refused or failed the read or write.
    Store(valence::Error),
}

impl std::fmt::Display for PhoneVerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidNumber => write!(f, "invalid phone number"),
            Self::TextNotConfigured => write!(f, "text messages are not available"),
            Self::TooSoon => write!(f, "a code was sent less than a minute ago"),
            Self::NoPendingCode => write!(f, "no verification code is waiting"),
            Self::Expired => write!(f, "verification code expired"),
            Self::WrongCode => write!(f, "wrong verification code"),
            Self::TooManyTries => write!(f, "too many wrong codes"),
            Self::Send(err) => write!(f, "send verification code: {err}"),
            Self::Store(err) => write!(f, "notification contact store: {err}"),
        }
    }
}

impl std::error::Error for PhoneVerificationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Send(err) => Some(err),
            Self::Store(err) => Some(err),
            _ => None,
        }
    }
}

impl From<valence::Error> for PhoneVerificationError {
    fn from(err: valence::Error) -> Self {
        Self::Store(err)
    }
}

/// `number` without spaces, dashes, dots or parentheses, when it is E.164:
/// `+`, a non-zero digit, then 7 to 14 more digits.
pub fn normalize_phone(number: &str) -> Option<String> {
    let n: String = number
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '.' | '(' | ')'))
        .collect();
    let digits = n.strip_prefix('+')?;
    let ok = (8..=15).contains(&digits.len())
        && digits.bytes().all(|b| b.is_ascii_digit())
        && !digits.starts_with('0');
    ok.then_some(n)
}

fn hash_code(salt: &str, code: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update(b":");
    h.update(code.as_bytes());
    hex::encode(h.finalize())
}

/// Compare without stopping at the first difference.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

async fn load(valence: &Valence, user: &RecordId) -> valence::Result<Option<NotificationContact>> {
    match NotificationContact::get(user.id(), valence, valence::use_!(r"When **notification delivery** sends a text, and on the **notification preferences page** when someone adds a phone number, we **read Notification Contact** to find that person's verified number or the code waiting for it. Only that person and the notification delivery service read the row.")).await {
        Err(valence::Error::Privacy(_)) => Ok(None),
        other => other,
    }
}

/// The phone `user` verified here, if any, read as the actor on `valence`.
/// A row the actor may not read counts as none.
pub(crate) async fn verified_phone(
    valence: &Valence,
    user: &RecordId,
) -> valence::Result<Option<String>> {
    Ok(load(valence, user).await?.and_then(|row| {
        row.phone_verified_at()
            .is_some()
            .then(|| row.phone().cloned())
            .flatten()
    }))
}

#[allow(clippy::too_many_arguments)]
fn contact_row(
    user: &RecordId,
    phone: Option<String>,
    phone_verified_at: Option<DateTime<Utc>>,
    pending_phone: Option<String>,
    code_hash: Option<String>,
    code_salt: Option<String>,
    code_expires_at: Option<DateTime<Utc>>,
    code_tries: i64,
    now: DateTime<Utc>,
) -> valence::Result<NotificationContact> {
    NotificationContact::new(
        user.clone(),
        phone,
        phone_verified_at,
        pending_phone,
        code_hash,
        code_salt,
        code_expires_at,
        code_tries,
        now,
    )
}

async fn save(valence: &Valence, user: &RecordId, row: NotificationContact) -> valence::Result<()> {
    NotificationContact::upsert(user.id(), row, valence, valence::use_!(r"On the **notification preferences page**, when someone adds or confirms a phone number, we **save Notification Contact** with the code waiting for it or the verified number, so **notification delivery** can text them. Only that person writes the row.")).await?;
    Ok(())
}

/// Text a 6-digit code to `number` and keep its hash for 10 minutes.
///
/// Runs as `user` on `valence`. A verified number already on file stays in
/// use until the new one is confirmed.
///
/// # Errors
///
/// [`PhoneVerificationError::InvalidNumber`],
/// [`TextNotConfigured`](PhoneVerificationError::TextNotConfigured),
/// [`TooSoon`](PhoneVerificationError::TooSoon), [`Send`](PhoneVerificationError::Send)
/// or [`Store`](PhoneVerificationError::Store).
pub async fn start_phone_verification(
    valence: &Valence,
    user: &RecordId,
    number: &str,
) -> Result<(), PhoneVerificationError> {
    start_phone_verification_at(valence, user, number, Utc::now()).await
}

/// [`start_phone_verification`] with an explicit clock.
///
/// # Errors
///
/// Same as [`start_phone_verification`].
pub async fn start_phone_verification_at(
    valence: &Valence,
    user: &RecordId,
    number: &str,
    now: DateTime<Utc>,
) -> Result<(), PhoneVerificationError> {
    let number = normalize_phone(number).ok_or(PhoneVerificationError::InvalidNumber)?;
    let channel = NotificationChannels::get(valence)
        .and_then(|c| c.channel(ChannelKind::Text).cloned())
        .ok_or(PhoneVerificationError::TextNotConfigured)?;
    let existing = load(valence, user).await?;
    if let Some(sent_at) = existing
        .as_ref()
        .and_then(|r| r.code_expires_at().copied())
        .map(|exp| exp - Duration::minutes(CODE_TTL_MINUTES))
    {
        if now < sent_at + Duration::seconds(RESEND_AFTER_SECONDS) {
            return Err(PhoneVerificationError::TooSoon);
        }
    }

    let (code, salt) = {
        let mut rng = rand::thread_rng();
        let code = format!("{:06}", rng.gen_range(0..1_000_000u32));
        let salt: [u8; 16] = rng.gen();
        (code, hex::encode(salt))
    };
    channel
        .deliver(
            &Contact::Phone(number.clone()),
            &OutgoingNotification {
                title: "Verification code".into(),
                message: format!(
                    "Your verification code is {code}. It expires in {CODE_TTL_MINUTES} minutes."
                ),
                url: None,
                kind_id: PHONE_CODE_KIND_ID.into(),
                notification_id: None,
            },
        )
        .await
        .map_err(PhoneVerificationError::Send)?;

    let (phone, verified_at) = existing.as_ref().map_or((None, None), |r| {
        (r.phone().cloned(), r.phone_verified_at().copied())
    });
    let row = contact_row(
        user,
        phone,
        verified_at,
        Some(number),
        Some(hash_code(&salt, &code)),
        Some(salt),
        Some(now + Duration::minutes(CODE_TTL_MINUTES)),
        0,
        now,
    )?;
    save(valence, user, row).await?;
    Ok(())
}

/// Check `code` and, when it matches, store the pending number as verified.
///
/// Runs as `user` on `valence`. Each wrong code counts a try; after
/// [`MAX_CODE_TRIES`] the code is refused even when right.
///
/// # Errors
///
/// [`PhoneVerificationError::NoPendingCode`], [`Expired`](PhoneVerificationError::Expired),
/// [`WrongCode`](PhoneVerificationError::WrongCode),
/// [`TooManyTries`](PhoneVerificationError::TooManyTries) or
/// [`Store`](PhoneVerificationError::Store).
pub async fn confirm_phone_verification(
    valence: &Valence,
    user: &RecordId,
    code: &str,
) -> Result<(), PhoneVerificationError> {
    confirm_phone_verification_at(valence, user, code, Utc::now()).await
}

/// [`confirm_phone_verification`] with an explicit clock.
///
/// # Errors
///
/// Same as [`confirm_phone_verification`].
pub async fn confirm_phone_verification_at(
    valence: &Valence,
    user: &RecordId,
    code: &str,
    now: DateTime<Utc>,
) -> Result<(), PhoneVerificationError> {
    let row = load(valence, user)
        .await?
        .ok_or(PhoneVerificationError::NoPendingCode)?;
    let (Some(pending), Some(hash), Some(salt), Some(expires_at)) = (
        row.pending_phone().cloned(),
        row.code_hash().cloned(),
        row.code_salt().cloned(),
        row.code_expires_at().copied(),
    ) else {
        return Err(PhoneVerificationError::NoPendingCode);
    };
    let tries = *row.code_tries();
    if tries >= MAX_CODE_TRIES {
        return Err(PhoneVerificationError::TooManyTries);
    }
    if now >= expires_at {
        return Err(PhoneVerificationError::Expired);
    }

    if !same(&hash_code(&salt, code.trim()), &hash) {
        let updated = contact_row(
            user,
            row.phone().cloned(),
            row.phone_verified_at().copied(),
            Some(pending),
            Some(hash),
            Some(salt),
            Some(expires_at),
            tries + 1,
            now,
        )?;
        save(valence, user, updated).await?;
        return Err(if tries + 1 >= MAX_CODE_TRIES {
            PhoneVerificationError::TooManyTries
        } else {
            PhoneVerificationError::WrongCode
        });
    }

    let verified = contact_row(
        user,
        Some(pending),
        Some(now),
        None,
        None,
        None,
        None,
        0,
        now,
    )?;
    save(valence, user, verified).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_phone_accepts_e164_and_strips_punctuation() {
        assert_eq!(
            normalize_phone("+1 (555) 123-4567").as_deref(),
            Some("+15551234567")
        );
        assert_eq!(
            normalize_phone("+353861234567").as_deref(),
            Some("+353861234567")
        );
    }

    #[test]
    fn normalize_phone_rejects_junk() {
        for bad in [
            "5551234567",
            "+0123456789",
            "+1234",
            "+1234567890123456",
            "+1555abc4567",
            "",
        ] {
            assert_eq!(normalize_phone(bad), None, "{bad}");
        }
    }

    #[test]
    fn hash_code_depends_on_salt_and_code() {
        let a = hash_code("s1", "123456");
        assert_eq!(a.len(), 64);
        assert_ne!(a, hash_code("s2", "123456"));
        assert_ne!(a, hash_code("s1", "123457"));
        assert!(same(&a, &hash_code("s1", "123456")));
        assert!(!same(&a, "short"));
    }
}
