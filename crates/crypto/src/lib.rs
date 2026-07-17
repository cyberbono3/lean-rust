//! Sync-core adapter for the leanSig post-quantum signature scheme.
//!
//! This crate is the dependency-inversion seam between consensus code and the
//! upstream signature implementation. Making that seam a crate rather than a
//! runtime module is deliberate — Cargo then enforces what this crate may reach
//! for, which a module split would leave to review discipline.
//!
//! Sync-core: signing and verification are pure functions of their inputs. This
//! crate uses no async runtime and does no logging or I/O — it returns errors and
//! the runtime layer decides what to log. Its direct dependencies are `leansig`,
//! `types`, `rand`, and `thiserror`; no `tokio`, `libp2p`, or `axum` appears
//! anywhere in its tree.
//!
//! One caveat, stated plainly because the layer rule is written in terms of
//! `cargo tree`: `tracing` *does* appear transitively, via leanSig's Plonky3
//! dependencies (`p3-dft` declares it unconditionally, with no feature to
//! disable it). No code here imports or emits it, and no other sync-core crate
//! pulls it today. Whether the rule should distinguish "must not use" from "must
//! not transitively pull" is an open decision, not something this crate settled
//! on its own.
//!
//! # Surface
//!
//! - [`generate`] — key generation. The only supported way to obtain a
//!   [`SigningKey`], which is what keeps leanSig's key types off this surface.
//! - [`SigningKey`] — a secret key plus the watermark that keeps one-time keys
//!   one-time. Signing takes `&mut self` so epoch reuse cannot happen by
//!   accident.
//! - [`verify`] — stateless verification, hence a free function.
//! - [`ProdScheme`] — the interop-pinned production scheme binding.
//! - [`SchemeWire`] — sealed; appears only as a bound.
//! - [`CryptoError`] — the crate's one error type.
//!
//! Epochs are opaque `u32` values here, exactly as leanSig treats them. The
//! mapping from consensus slots to signature epochs belongs to the runtime layer.
//!
//! # Wire types
//!
//! [`Signature`] and [`PublicKey`] are re-exported from `types`, never redefined
//! — the wire types have one owner.
//!
//! [`Signature`] is a **padded container**: it is wider than the signature it
//! carries, and the trailing bytes are zero padding that verification slices off.
//! That padding is not authenticated. Callers must not read meaning into the
//! region beyond the payload.
//!
//! # Layer contract
//!
//! - The only internal-workspace dependency is `types` (plus upstream leanSig).
//! - The only permitted consumers are the runtime layer and the offline keygen
//!   tooling. Cargo does not enforce this direction — it is checked by review and
//!   by `cargo tree -i -p crypto`.
//! - This crate names no `ssz` trait directly. leanSig implements a different
//!   `ethereum_ssz` major than the workspace `ssz` crate, so all encoding goes
//!   through leanSig's own serialization surface.
#![forbid(unsafe_code)]

// Modules are crate-private; the public surface is the re-exports below. Keeping
// the API in one place is the architecture rule, and it also means module layout
// stays an implementation detail rather than a compatibility promise.
pub(crate) mod error;
pub(crate) mod key_state;
pub(crate) mod record;
pub(crate) mod scheme;
pub(crate) mod sign;
pub(crate) mod verify;

pub use error::CryptoError;
pub use key_state::SigningKey;
pub use scheme::{generate, ProdScheme, SchemeWire, PROD_LIFETIME};
pub use verify::verify;

/// The XMSS wire public key. Re-exported from `types` — not redefined here.
pub use types::PublicKey;
/// The XMSS wire signature container. Re-exported from `types` — not redefined
/// here.
pub use types::Signature;
/// The crypto-free, persistable OTS key-state record and its decode error (SA5).
/// Re-exported from `types` — consumers reach the record through this crate
/// alongside the key types and the [`SigningKey::to_record`] / `from_record` pair.
pub use types::{OtsKeyState, OtsKeyStateDecodeError};
