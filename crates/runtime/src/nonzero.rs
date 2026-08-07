//! Const-context `NonZeroUsize` construction.
//!
//! One home for the `nz` idiom, shared by [`crate::sync`]'s config and
//! [`crate::p2p`]'s admission bound. Both define non-zero associated constants;
//! a second copy is one more thing to keep in sync for no benefit.

use core::num::NonZeroUsize;

/// Builds a [`NonZeroUsize`] from a literal at compile time; panics if the input
/// is zero.
///
/// Exists because the workspace denies `clippy::unwrap_used`, which applies
/// inside `const fn` as well — so the otherwise-obvious
/// `NonZeroUsize::new(n).unwrap()` cannot be written here. This is NOT an MSRV
/// workaround: const `unwrap` has been stable since 1.83 and this workspace pins
/// 1.87. The lint policy is the binding constraint, and it has no expiry.
///
/// The `panic!` arm needs no `#[allow]` under the workspace `clippy::panic =
/// deny`: clippy exempts that lint inside `const fn` (verified on the pinned
/// 1.87.0 — an implementation detail, not a stability promise), and every call
/// site here is a const initializer, so the arm is unreachable at runtime.
pub(crate) const fn nz(n: usize) -> NonZeroUsize {
    match NonZeroUsize::new(n) {
        Some(v) => v,
        None => panic!("expected non-zero constant"),
    }
}
