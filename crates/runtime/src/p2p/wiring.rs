//! Compile-time wiring proofs for the p2p crate.
//!
//! Asserts that [`crate::p2p::service::P2pService`] satisfies
//! [`crate::core::Service`]. The host construction path stays
//! object-safe so a downstream composition root can hold it as
//! `Arc<dyn crate::core::Service>`.

use static_assertions::assert_impl_all;

assert_impl_all!(crate::p2p::service::P2pService: crate::core::Service);
