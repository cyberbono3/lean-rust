//! Sync-core adapter home for the leanSig post-quantum signature scheme.
//!
//! This crate is the dependency-inversion seam between consensus code and the
//! upstream signature implementation: the runtime layer depends on the port
//! traits declared here, never on leanSig internals. Making that seam a crate
//! rather than a runtime module is deliberate — Cargo then enforces what this
//! crate may reach for, which a module split would leave to review discipline.
//! The reverse direction is not mechanical: nothing stops another crate from
//! adding a dependency on this one, so the consumer restriction below is a
//! review convention.
//!
//! Sync-core: no `tokio`, `tracing`, `libp2p`, or `axum`. Signing and
//! verification are pure functions of their inputs; the runtime layer owns
//! scheduling, logging, and I/O.
//!
//! # Scope
//!
//! The crate is a scaffold today — it carries no signing logic yet. Follow-up
//! work fills these module homes:
//!
//! - `sign` / `verify` — the signer and verifier port traits. Two narrow
//!   surfaces rather than one combined trait: verification is stateless, while
//!   signing advances a one-time-signature index and so needs `&mut self`.
//! - `key_state` — the algorithmic one-time-signature key state, plus its
//!   conversion to and from a plain byte record that `types` will own. Placing
//!   that record in `types` is what will let `storage` persist key state
//!   without taking a dependency on this crate.
//! - `error` — the crate's `thiserror` error enum.
//!
//! # Layer contract
//!
//! - The only internal-workspace dependency is `types` (plus the upstream
//!   leanSig crate, once the adapter lands).
//! - The only permitted consumers are `runtime` and the offline `lean-cli`
//!   keygen tooling. `protocol`, `forkchoice`, `storage`, and `networking`
//!   must not depend on this crate. Cargo does not enforce this direction —
//!   it is checked by review and by `cargo tree -i -p crypto`.
//! - A later change re-exports the signature and public-key newtypes from
//!   `types` rather than redefining them here — the wire types have one owner.
#![forbid(unsafe_code)]
