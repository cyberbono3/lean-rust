//! Compile-time wiring proofs for the p2p crate.
//!
//! Asserts that [`crate::service::P2pService`] satisfies
//! [`lean_core::Service`]. The host construction path stays
//! object-safe so a downstream composition root can hold it as
//! `Arc<dyn lean_core::Service>`.

use static_assertions::assert_impl_all;

assert_impl_all!(crate::service::P2pService: lean_core::Service);
