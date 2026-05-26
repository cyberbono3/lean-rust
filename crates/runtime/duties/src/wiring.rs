//! Compile-time wiring proofs for the duties crate.
//!
//! Asserts that [`lean_chain::Service`] satisfies the
//! [`crate::Chain`] port. The [`crate::Publisher`] port has no
//! in-crate impl (per Decision 7 / Issue #37); the `node` crate
//! provides the libp2p-backed adapter, and the static assertion for
//! that lives there.

use static_assertions::assert_impl_all;

assert_impl_all!(lean_chain::Service: crate::ports::Chain);
