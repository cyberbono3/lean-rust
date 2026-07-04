//! Compile-time wiring proofs for the duties crate.
//!
//! The `Chain` / `Publisher` port traits were collapsed to concrete
//! types, so the surviving proof is that the concrete [`crate::duties::Service`]
//! still implements the [`crate::core::Service`] lifecycle trait and is
//! shareable across tasks (`Send + Sync`) — the composition root stores
//! it behind `Arc<dyn crate::core::Service>`.

use static_assertions::assert_impl_all;

assert_impl_all!(crate::duties::Service: crate::core::Service, Send, Sync);
