//! Compile-time wiring proofs for the duties crate.
//!
//! The `Chain` / `Publisher` port traits were collapsed to concrete
//! types, so the surviving proof is that the concrete [`crate::Service`]
//! still implements the [`lean_core::Service`] lifecycle trait and is
//! shareable across tasks (`Send + Sync`) — the composition root stores
//! it behind `Arc<dyn lean_core::Service>`.

use static_assertions::assert_impl_all;

assert_impl_all!(crate::Service: lean_core::Service, Send, Sync);
