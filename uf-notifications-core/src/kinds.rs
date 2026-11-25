//! Notification kind registry and per-person channel preferences.
//!
//! Each app registers the kinds it sends with [`inventory::submit!`], next to
//! its `uf_app!` registration, the same way the bell registers its shell
//! contribution:
//!
//! ```rust
//! use uf_notifications_core::{Channels, NotificationKind};
//!
//! inventory::submit! {
//!     NotificationKind {
//!         id: "magnon.assigned",
//!         app: "magnon",
//!         label: "Assigned to you",
//!         description: "Someone assigned you a task.",
//!         defaults: Channels { in_app: true, email: true, text: false },
//!         asks_action: true,
//!         urgent: false,
//!     }
//! }
//! ```
//!
//! People change the defaults one kind at a time. [`set_channel`] stores the
//! change, [`effective_channels`] answers what a kind should use for one
//! person, and [`load_settings`] / [`save_settings`] hold digest and quiet
//! hours. Every read and write runs as the caller through Valence privacy:
//! a person can only see and change their own rows.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use chrono::Utc;
use valence::{Model, RecordId, RecordPredicate, Valence};

use crate::generated::{NotificationPreference, NotificationSettings, NotificationSettingsDigest};

/// Which channels a notification kind reaches a person on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Channels {
    /// The bell and inbox.
    pub in_app: bool,
    /// Email to the person's verified address.
    pub email: bool,
    /// Text message to the person's verified phone.
    pub text: bool,
}

impl Channels {
    /// In-app only: what an unregistered kind gets.
    pub const IN_APP_ONLY: Self = Self {
        in_app: true,
        email: false,
        text: false,
    };

    /// Whether `channel` is on.
    pub fn get(self, channel: Channel) -> bool {
        match channel {
            Channel::InApp => self.in_app,
            Channel::Email => self.email,
            Channel::Text => self.text,
        }
    }

    /// Turn `channel` on or off.
    pub fn set(&mut self, channel: Channel, on: bool) {
        match channel {
            Channel::InApp => self.in_app = on,
            Channel::Email => self.email = on,
            Channel::Text => self.text = on,
        }
    }
}

/// One delivery channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    /// The bell and inbox.
    InApp,
    /// Email.
    Email,
    /// Text message.
    Text,
}

/// A notification kind an app sends, registered with [`inventory::submit!`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotificationKind {
    /// `<app>.<name>`, for example `magnon.assigned`. This is the `kind`
    /// senders pass to [`send_notification`](crate::send_notification).
    pub id: &'static str,
    /// The sending app's id, for example `magnon`.
    pub app: &'static str,
    /// Short label the bell and preferences page show.
    pub label: &'static str,
    /// One sentence on when this kind is sent.
    pub description: &'static str,
    /// Channels used until the person changes them.
    pub defaults: Channels,
    /// The notification asks the reader to do something (assignments,
    /// mentions, approvals, invitations).
    pub asks_action: bool,
    /// Skips a digest and goes out right away (approvals, access requests).
    pub urgent: bool,
}

inventory::collect!(NotificationKind);

/// Every registered kind, sorted by app, then label.
pub fn kinds() -> Vec<&'static NotificationKind> {
    let mut all: Vec<&'static NotificationKind> =
        inventory::iter::<NotificationKind>.into_iter().collect();
    all.sort_by(|a, b| (a.app, a.label, a.id).cmp(&(b.app, b.label, b.id)));
    all
}

/// The registered kind with this id, if any.
pub fn kind(id: &str) -> Option<&'static NotificationKind> {
    inventory::iter::<NotificationKind>
        .into_iter()
        .find(|k| k.id == id)
}

/// Registered kinds for one app, sorted by label.
pub fn kinds_for_app(app: &str) -> Vec<&'static NotificationKind> {
    kinds().into_iter().filter(|k| k.app == app).collect()
}

/// Log one `warn` the first time an unregistered `kind` is sent. Returns
/// whether this call logged.
pub(crate) fn warn_unregistered_once(kind: &str) -> bool {
    static WARNED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    let first = WARNED
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(kind.to_string());
    if first {
        tracing::warn!(
            operation = "send",
            kind,
            "notification kind is not registered; storing it in-app only"
        );
    }
    first
}

/// Whether a person's email address and phone are verified, implemented by the host.
///
/// Email and text stay off in [`effective_channels`] until these say yes.
/// Everything defaults to "not verified". Email verification belongs to the
/// host (Lepton); a phone verified through
/// [`start_phone_verification`](crate::phone::start_phone_verification) counts
/// whatever [`phone_verified`](Self::phone_verified) answers.
///
/// Check as the actor on `valence`; never elevate to System. Delivery calls
/// these as the `svc:notifications` service actor
/// ([`delivery_actor`](crate::delivery_policy::delivery_actor)) and the digest
/// job as System, so the host's lookup must answer for those actors too.
#[async_trait::async_trait]
pub trait ContactVerification: Send + Sync {
    /// Whether `user` has a verified email address.
    ///
    /// # Errors
    ///
    /// Returns the underlying Valence error when the lookup fails.
    async fn email_verified(&self, user: &RecordId, valence: &Valence) -> valence::Result<bool> {
        let _ = (user, valence);
        Ok(false)
    }

    /// Whether `user` has a verified phone number the host verified itself.
    ///
    /// # Errors
    ///
    /// Returns the underlying Valence error when the lookup fails.
    async fn phone_verified(&self, user: &RecordId, valence: &Valence) -> valence::Result<bool> {
        let _ = (user, valence);
        Ok(false)
    }

    /// `user`'s verified email address, used to deliver email. `None` (the
    /// default) means email is never sent even when verified.
    ///
    /// # Errors
    ///
    /// Returns the underlying Valence error when the lookup fails.
    async fn verified_email(
        &self,
        user: &RecordId,
        valence: &Valence,
    ) -> valence::Result<Option<String>> {
        let _ = (user, valence);
        Ok(None)
    }

    /// A phone number the host verified itself, in E.164, used when the
    /// person has no number verified through this crate. Defaults to `None`.
    ///
    /// # Errors
    ///
    /// Returns the underlying Valence error when the lookup fails.
    async fn verified_phone(
        &self,
        user: &RecordId,
        valence: &Valence,
    ) -> valence::Result<Option<String>> {
        let _ = (user, valence);
        Ok(None)
    }
}

fn verification_slot() -> &'static RwLock<Option<Arc<dyn ContactVerification>>> {
    static SLOT: OnceLock<RwLock<Option<Arc<dyn ContactVerification>>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Install the host's [`ContactVerification`]. Call at host boot; a second
/// call replaces the first.
pub fn register_contact_verification(check: Arc<dyn ContactVerification>) {
    *verification_slot()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(check);
}

fn verification_check() -> Option<Arc<dyn ContactVerification>> {
    verification_slot()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Whether `user`'s email and phone are verified, as `(email, phone)`.
///
/// Email comes from the registered [`ContactVerification`]. The phone counts
/// as verified when the person verified one through
/// [`start_phone_verification`](crate::phone::start_phone_verification), or
/// when the host's check says so. Without a registered check, only a phone
/// verified here counts.
///
/// # Errors
///
/// Returns the Valence error from the contact read or the host's check.
pub async fn contact_verified(user: &RecordId, valence: &Valence) -> valence::Result<(bool, bool)> {
    let own_phone = crate::phone::verified_phone(valence, user).await?.is_some();
    match verification_check() {
        Some(check) => Ok((
            check.email_verified(user, valence).await?,
            own_phone || check.phone_verified(user, valence).await?,
        )),
        None => Ok((false, own_phone)),
    }
}

/// Where to deliver `channel` to `user`: the host's verified email, or the
/// phone verified here, else the host's verified phone.
///
/// # Errors
///
/// Returns the Valence error from the contact read or the host's check.
pub(crate) async fn verified_contact(
    user: &RecordId,
    valence: &Valence,
    channel: crate::channels::ChannelKind,
) -> valence::Result<Option<crate::channels::Contact>> {
    use crate::channels::{ChannelKind, Contact};
    let check = verification_check();
    match channel {
        ChannelKind::Email => match check {
            Some(check) if check.email_verified(user, valence).await? => Ok(check
                .verified_email(user, valence)
                .await?
                .map(Contact::Email)),
            _ => Ok(None),
        },
        ChannelKind::Text => {
            if let Some(phone) = crate::phone::verified_phone(valence, user).await? {
                return Ok(Some(Contact::Phone(phone)));
            }
            match check {
                Some(check) if check.phone_verified(user, valence).await? => Ok(check
                    .verified_phone(user, valence)
                    .await?
                    .map(Contact::Phone)),
                _ => Ok(None),
            }
        }
    }
}

/// How often email goes out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Digest {
    /// One email per notification.
    #[default]
    Instant,
    /// One email a day.
    Daily,
    /// One email a week.
    Weekly,
}

impl From<&NotificationSettingsDigest> for Digest {
    fn from(d: &NotificationSettingsDigest) -> Self {
        match d {
            NotificationSettingsDigest::Instant => Self::Instant,
            NotificationSettingsDigest::Daily => Self::Daily,
            NotificationSettingsDigest::Weekly => Self::Weekly,
        }
    }
}

impl From<Digest> for NotificationSettingsDigest {
    fn from(d: Digest) -> Self {
        match d {
            Digest::Instant => Self::Instant,
            Digest::Daily => Self::Daily,
            Digest::Weekly => Self::Weekly,
        }
    }
}

/// A person's digest and quiet-hours settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// How often email goes out.
    pub digest: Digest,
    /// Whether quiet hours hold email and text.
    pub quiet_enabled: bool,
    /// Start of quiet hours, local `"HH:MM"`.
    pub quiet_start: String,
    /// End of quiet hours, local `"HH:MM"`.
    pub quiet_end: String,
    /// IANA time zone the quiet hours are in, for example `"Europe/Dublin"`.
    pub time_zone: String,
}

impl Default for Settings {
    /// Instant email, quiet hours off (22:00–07:00 once turned on), UTC.
    fn default() -> Self {
        Self {
            digest: Digest::Instant,
            quiet_enabled: false,
            quiet_start: "22:00".into(),
            quiet_end: "07:00".into(),
            time_zone: "UTC".into(),
        }
    }
}

/// Longest `time_zone` [`save_settings`] accepts.
pub const MAX_TIME_ZONE_CHARS: usize = 64;

/// Failure from the preference read and write helpers.
#[derive(Debug)]
pub enum PreferenceError {
    /// The kind id is not registered.
    UnknownKind,
    /// Settings failed validation (times not `"HH:MM"`, bad time zone).
    InvalidSettings(&'static str),
    /// Valence refused or failed the read or write.
    Store(valence::Error),
}

impl std::fmt::Display for PreferenceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownKind => write!(f, "unknown notification kind"),
            Self::InvalidSettings(why) => write!(f, "invalid notification settings: {why}"),
            Self::Store(err) => write!(f, "notification preference store: {err}"),
        }
    }
}

impl std::error::Error for PreferenceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(err) => Some(err),
            _ => None,
        }
    }
}

impl From<valence::Error> for PreferenceError {
    fn from(err: valence::Error) -> Self {
        Self::Store(err)
    }
}

/// Record id for one person's preference row for one kind. The kind is
/// hex-encoded so ids stay plain and two kinds never share a row.
fn preference_id(user: &RecordId, kind_id: &str) -> String {
    use std::fmt::Write as _;
    let mut id = format!("pref-{}-", user.id());
    for b in kind_id.bytes() {
        let _ = write!(id, "{b:02x}");
    }
    id
}

/// Every channel choice `user` has saved, as `(kind_id, channels)`. Kinds
/// with no row use their defaults.
///
/// # Errors
///
/// Returns the Valence error when the query fails.
pub async fn load_preferences(
    valence: &Valence,
    user: &RecordId,
) -> valence::Result<Vec<(String, Channels)>> {
    let rows: Vec<NotificationPreference> = NotificationPreference::query(valence, valence::use_!(r"On the **notification preferences page**, and when **notification delivery** decides where to send, we **list Notification Preference** rows for the signed-in person so their own channel choices replace the defaults. Only that person reads these rows."))
        .where_user(RecordPredicate::Equals(user.clone()))
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.kind_id().clone(),
                Channels {
                    in_app: *r.in_app(),
                    email: *r.email(),
                    text: *r.text(),
                },
            )
        })
        .collect())
}

/// `user`'s saved channels for `kind_id`, else `defaults`.
async fn stored_or_default(
    valence: &Valence,
    user: &RecordId,
    kind_id: &str,
    defaults: Channels,
) -> valence::Result<Channels> {
    let row = NotificationPreference::get(&preference_id(user, kind_id), valence, valence::use_!(r"When **notification delivery** or the **notification preferences page** needs one kind's setting, we **read Notification Preference** for the signed-in person so their own choice replaces the default. Only that person reads the row."))
        .await?;
    Ok(row.map_or(defaults, |r| Channels {
        in_app: *r.in_app(),
        email: *r.email(),
        text: *r.text(),
    }))
}

/// Turn email or text off unless the matching contact is verified.
pub fn mask_unverified(
    mut channels: Channels,
    email_verified: bool,
    phone_verified: bool,
) -> Channels {
    channels.email &= email_verified;
    channels.text &= phone_verified;
    channels
}

/// The channels `kind_id` should use for `user`.
///
/// The saved row wins, else the kind's defaults; an unregistered kind is
/// in-app only. Email stays off until the email is verified and text until
/// a phone is, per the host's [`ContactVerification`]. Read as the actor on
/// `valence`, which must be `user`.
///
/// # Errors
///
/// Returns the Valence error when the preference read or verification check
/// fails, so a store failure never silently falls back to defaults.
pub async fn effective_channels(
    valence: &Valence,
    user: &RecordId,
    kind_id: &str,
) -> valence::Result<Channels> {
    let Some(kind) = kind(kind_id) else {
        return Ok(Channels::IN_APP_ONLY);
    };
    let stored = stored_or_default(valence, user, kind_id, kind.defaults).await?;
    let (email_ok, phone_ok) = contact_verified(user, valence).await?;
    Ok(mask_unverified(stored, email_ok, phone_ok))
}

/// Turn one channel on or off for one kind and save it for `user`.
///
/// The first change for a kind starts from its defaults. Returns the saved
/// channels (before verification masking).
///
/// # Errors
///
/// [`PreferenceError::UnknownKind`] for an unregistered kind id, or
/// [`PreferenceError::Store`] when Valence refuses or fails the write (for
/// example, when the actor is not `user`).
pub async fn set_channel(
    valence: &Valence,
    user: &RecordId,
    kind_id: &str,
    channel: Channel,
    on: bool,
) -> Result<Channels, PreferenceError> {
    let kind = kind(kind_id).ok_or(PreferenceError::UnknownKind)?;
    let mut channels = stored_or_default(valence, user, kind_id, kind.defaults).await?;
    channels.set(channel, on);
    let row = NotificationPreference::new(
        user.clone(),
        kind.id.to_string(),
        channels.in_app,
        channels.email,
        channels.text,
        Utc::now(),
    )?;
    NotificationPreference::upsert(&preference_id(user, kind_id), row, valence, valence::use_!(r"On the **notification preferences page**, we **save Notification Preference** when the signed-in person switches a channel for one kind, so **notification delivery** follows their choice. Only that person writes the row.")).await?;
    Ok(channels)
}

/// `user`'s digest and quiet-hours settings; [`Settings::default`] when none are saved.
///
/// # Errors
///
/// Returns the Valence error when the read fails.
pub async fn load_settings(valence: &Valence, user: &RecordId) -> valence::Result<Settings> {
    let row = NotificationSettings::get(user.id(), valence, valence::use_!(r"On the **notification preferences page**, and when **notification delivery** times email and text, we **read Notification Settings** for the signed-in person to apply their digest and quiet hours. Only that person reads the row."))
        .await?;
    Ok(row.map_or_else(Settings::default, |r| Settings {
        digest: r.digest().into(),
        quiet_enabled: *r.quiet_enabled(),
        quiet_start: r.quiet_start().clone(),
        quiet_end: r.quiet_end().clone(),
        time_zone: r.time_zone().clone(),
    }))
}

/// Whether `s` is a 24-hour `"HH:MM"` time.
pub(crate) fn is_hh_mm(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 5
        && b[2] == b':'
        && b[..2].iter().chain(&b[3..]).all(u8::is_ascii_digit)
        && (b[0] - b'0') * 10 + (b[1] - b'0') < 24
        && b[3] < b'6'
}

/// Save `user`'s digest and quiet-hours settings.
///
/// # Errors
///
/// [`PreferenceError::InvalidSettings`] when a quiet-hours time is not
/// `"HH:MM"` or `time_zone` is empty or longer than [`MAX_TIME_ZONE_CHARS`];
/// [`PreferenceError::Store`] when Valence refuses or fails the write.
pub async fn save_settings(
    valence: &Valence,
    user: &RecordId,
    settings: Settings,
) -> Result<(), PreferenceError> {
    if !is_hh_mm(&settings.quiet_start) || !is_hh_mm(&settings.quiet_end) {
        return Err(PreferenceError::InvalidSettings(
            "quiet hours must be HH:MM",
        ));
    }
    let tz = settings.time_zone.trim();
    if tz.is_empty() || tz.chars().count() > MAX_TIME_ZONE_CHARS {
        return Err(PreferenceError::InvalidSettings("time zone"));
    }
    // Keep when the last digest went out, so changing settings never resends it.
    let last_digest_at = NotificationSettings::get(user.id(), valence, valence::use_!(r"On the **notification preferences page**, before saving the signed-in person's digest or quiet hours, we **read Notification Settings** to keep when their last digest went out. Only that person reads the row."))
        .await?
        .and_then(|r| r.last_digest_at().copied());
    let row = NotificationSettings::new(
        user.clone(),
        settings.digest.into(),
        settings.quiet_enabled,
        settings.quiet_start,
        settings.quiet_end,
        tz.to_string(),
        Utc::now(),
        last_digest_at,
    )?;
    NotificationSettings::upsert(user.id(), row, valence, valence::use_!(r"On the **notification preferences page**, we **save Notification Settings** when the signed-in person changes their digest or quiet hours, so **notification delivery** follows them. Only that person writes the row.")).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    inventory::submit! {
        NotificationKind {
            id: "kindtest.zeta",
            app: "kindtest",
            label: "Zeta",
            description: "Unit-test kind.",
            defaults: Channels { in_app: true, email: true, text: false },
            asks_action: true,
            urgent: false,
        }
    }

    inventory::submit! {
        NotificationKind {
            id: "kindtest.alpha",
            app: "kindtest",
            label: "Alpha",
            description: "Unit-test kind.",
            defaults: Channels::IN_APP_ONLY,
            asks_action: false,
            urgent: true,
        }
    }

    #[test]
    fn kind_lookup_finds_registered_and_misses_unknown() {
        let k = kind("kindtest.zeta").expect("registered");
        assert_eq!(k.app, "kindtest");
        assert_eq!(k.label, "Zeta");
        assert!(k.defaults.email);
        assert!(kind("kindtest.missing").is_none());
    }

    #[test]
    fn kinds_for_app_sorts_by_label() {
        let ids: Vec<_> = kinds_for_app("kindtest").iter().map(|k| k.id).collect();
        assert_eq!(ids, ["kindtest.alpha", "kindtest.zeta"]);
        assert_eq!(kinds_for_app("no-such-app").len(), 0);
        assert!(kinds().len() >= 2);
    }

    #[test]
    fn warn_unregistered_once_warns_first_time_only() {
        assert!(warn_unregistered_once("kindtest.unregistered-once"));
        assert!(!warn_unregistered_once("kindtest.unregistered-once"));
    }

    #[test]
    fn mask_unverified_turns_off_email_and_text() {
        let all = Channels {
            in_app: true,
            email: true,
            text: true,
        };
        assert_eq!(mask_unverified(all, false, false), Channels::IN_APP_ONLY);
        assert_eq!(mask_unverified(all, true, true), all);
    }

    #[test]
    fn hh_mm_accepts_times_and_rejects_junk() {
        for ok in ["00:00", "07:30", "23:59"] {
            assert!(is_hh_mm(ok), "{ok}");
        }
        for bad in ["24:00", "7:30", "07:60", "ab:cd", "07-30", ""] {
            assert!(!is_hh_mm(bad), "{bad}");
        }
    }

    #[test]
    fn preference_id_is_distinct_per_kind() {
        let user = RecordId::new("user", "alice");
        assert_ne!(preference_id(&user, "a.b_c"), preference_id(&user, "a_b.c"));
        assert!(preference_id(&user, "a.b").starts_with("pref-alice-"));
    }
}
