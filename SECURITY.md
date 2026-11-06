# Security Policy

## Supported versions

Security fixes are accepted against the latest published `0.3.x` line of this
repository's crates (`uf-notifications-core`, `uf-notifications-api`). The
Orbital UI crate (`uf-notifications`) ships from unified-field-product.

## Reporting a vulnerability

Please **do not** open a public GitHub issue for security-sensitive reports.

Prefer one of the following:

1. **GitHub Security Advisories** — use
   [Report a vulnerability](https://github.com/unified-field-dev/uf-notifications/security/advisories/new)
   on this repository when available.
2. Contact the maintainers privately via the repository owner listed at
   https://github.com/unified-field-dev/uf-notifications.

Include:

- a description of the issue and its impact
- steps to reproduce or a proof of concept when possible
- affected crate names and versions

We will acknowledge receipt as soon as practical and coordinate a fix and
disclosure timeline with you.

## Scope

In scope: vulnerabilities in this repository's crates and documentation that
could cause unsafe production defaults, plus CI/supply-chain issues in this
repository.

Out of scope: vulnerabilities solely in third-party dependencies unless this
project mishandles them in a security-relevant way.

## Inbox notes

- **Create** allows Valence `SYSTEM_ONLY` (jobs, seeds) and
  `SCOPED_NOTIFICATION_CREATE`, which runs as the sending actor with no System
  elevation. `send_notification` records the acting Valence's actor as
  `sender`; callers never pass one. For a row with a `subject` (`table:id`),
  every check must pass:
  1. `sender` names the actor. A service user passes only when the sender is
     that service user.
  2. The actor can read `subject` under its own privacy.
  3. The recipient is in the subject's audience, answered by the check the host
     registered with `register_notification_audience(table, kind, check)`. With
     no check registered for that table, only a notification to the actor
     itself is allowed; every other recipient is refused.
- **Audience checks** answer yes or no for one recipient. They run with the
  caller's Valence, must not elevate to System, and must never return or log
  the audience list. An error from a check denies the create. The registry is
  process-wide; hosts fill it at boot. A later registration for the same table
  and kind replaces the earlier one and logs a `warn`, so any crate in the host
  can still swap another product's check until the registry moves into Valence.
- **Known gap (phase A):** a row without a `subject` is still allowed for any
  authenticated actor (a `sender`, if set, must still be the actor), with a
  `warn` log naming the kind, so existing senders
  keep working while they move to scoped sends. Until phase B removes that path,
  any signed-in actor can still write an unscoped notification to any user's
  inbox.
- **System-started senders** such as Chronon scripts and Catalyst rule actions
  (`cx-notify-action`) still create as System. That is the job's own actor, not
  a mid-request elevation. Delete stays `SYSTEM_ONLY`, and there is still no
  production session create server function.
- **Read and mark-read/unread** are `OWNER_BY_USER_FIELD` plus a Higgs session
  on the `#[server]` paths. A guessed notification UUID must not leak another
  user's row.
- **`dev-tools`** compiles `create_test_notification`. Leave that feature off
  of `default`, `ssr`, and `hydrate`. Production hosts must not enable it.
- Action URLs are same-origin relative paths (`sanitize_notification_url`).
  List/page `limit` is capped; count queries load at most 500 rows.
- Photon topic `user.notifications` is keyed by recipient; the unread badge
  WebSocket uses `auth = "user"`.
