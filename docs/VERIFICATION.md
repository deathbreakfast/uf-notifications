# Verification

Local commands for this domain workspace. GitHub Actions (`.github/workflows/ci.yml`)
runs the core/api + mount-host subset below on every push and PR.

This workspace pins `rust-toolchain.toml` to `nightly` (Leptos `nightly` features).

```bash
export CARGO_BUILD_JOBS=1
export CARGO_TARGET_DIR=target-uf-notifications

cargo test -p uf-notifications-core
cargo test -p uf-notifications-core --test workspace_members --test product_surface --test privacy_policy_integration
cargo test -p uf-notifications-api
cargo test -p uf-notifications-api --features ssr --lib
cargo test -p uf-notifications-api --features ssr --test api_ops_integration

# Teaching host (Axum oneshot):
cargo check -p notifications-mount-host
cargo run -p notifications-mount-host
cargo run -p uf-notifications-core --example system_mint

# Rustdoc link deny (workspace Cargo.toml allows broken_intra_doc_links by default):
RUSTDOCFLAGS="-D rustdoc::broken-intra-doc-links" cargo doc -p uf-notifications-core --no-deps
RUSTDOCFLAGS="-D rustdoc::broken-intra-doc-links" cargo doc -p uf-notifications-api --features ssr --no-deps
```

UI package (`uf-notifications`) and Playwright live in
`unified-field-product` — verify there (`cargo check -p uf-notifications --features ssr`,
`cargo check -p uf-product-ui-e2e --features ssr`, then product `docs/VERIFICATION.md`
Layer 2 for `notifications.spec.ts`).

### leptos-lints (CI job `leptos-lints`)

Needs `cargo-dylint` / `dylint-link` 6.0.1 and toolchain `nightly-2025-05-14`
(see `.github/workflows/ci.yml`). Hydrate API crate (`--no-deps`):

```bash
# cargo install cargo-dylint --locked --version 6.0.1
# cargo install dylint-link --locked --version 6.0.1
# rustup toolchain install nightly-2025-05-14 --component rustc-dev,llvm-tools-preview
export CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=fallback
# Prefer lepton's stdarch crate-attr on the pinned dylint nightly; on newer
# host nightlies you may need `-A stable_features` instead/in addition.
export RUSTFLAGS="-D warnings -Zcrate-attr=feature(stdarch_x86_avx512)"
cargo dylint --all -p uf-notifications-api --no-deps -- --features hydrate
```

Teaching host: [`examples/notifications-mount-host`](../examples/notifications-mount-host/).
Success line: `notifications_mount_host: OK — /notifications protect + inventory + bell slot`.
Hydrate/browser is out of gate for the oneshot.

## Rustdoc policy notes

| Package | `missing_docs` | Notes |
|---------|----------------|-------|
| `uf-notifications-core` | workspace deny | Primary Leptos-free library surface |
| `uf-notifications-api` | `#![allow(missing_docs)]` | Server-fn crate; ratchet item docs over time |

Use a dedicated `CARGO_TARGET_DIR` so parallel Unified Field checkouts do not collide.
