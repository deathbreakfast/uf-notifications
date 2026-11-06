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

- **Create** allows Valence `SYSTEM_ONLY` (jobs/seeds) and `AUTHENTICATED`
  (product side effects such as gauge request fanout under the session actor).
  There is still no production session create server function; delete stays
  `SYSTEM_ONLY`. Call `send_notification` from backend code with the request
  actor when possible so hosts do not mid-request elevate to System.
- **Known gap:** `AUTHENTICATED` create means any signed-in actor can write to
  any user's inbox. A create policy scoped to the actor's own permissions (the
  sender recorded as the actor, recipients limited by what the actor can see)
  will replace it.
- **Read and mark-read/unread** are `OWNER_BY_USER_FIELD` plus a Higgs session
  on the `#[server]` paths. A guessed notification UUID must not leak another
  user's row.
- **`dev-tools`** compiles `create_test_notification`. Leave that feature off
  of `default`, `ssr`, and `hydrate`. Production hosts must not enable it.
- Action URLs are same-origin relative paths (`sanitize_notification_url`).
  List/page `limit` is capped; count queries load at most 500 rows.
- Photon topic `user.notifications` is keyed by recipient; the unread badge
  WebSocket uses `auth = "user"`.
