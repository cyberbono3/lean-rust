//! Sync-core adapter for the leanSig post-quantum signature scheme.
//!
//! This crate is the dependency-inversion seam between consensus code and the
//! upstream signature implementation. Making that seam a crate rather than a
//! runtime module is deliberate — Cargo then enforces what this crate may reach
//! for, which a module split would leave to review discipline.
//!
//! Sync-core: no `tokio`, `tracing`, `libp2p`, or `axum`. Signing and
//! verification are pure functions of their inputs; the runtime layer owns
//! scheduling, logging, and I/O.
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

pub mod error;
pub mod key_state;
pub mod sign;
pub mod verify;

pub(crate) mod scheme;

pub use error::CryptoError;
pub use key_state::SigningKey;
pub use scheme::{generate, ProdScheme, SchemeWire};
pub use verify::verify;

/// The XMSS wire public key. Re-exported from `types` — not redefined here.
pub use types::PublicKey;
/// The XMSS wire signature container. Re-exported from `types` — not redefined
/// here.
pub use types::Signature;
