//! Lean HTTP API for the runtime shell.
//!
//! Read-only adapter over [`storage::Store`]. The crate hosts the
//! serde-tagged wire shapes for the runtime; domain types in `protocol`
//! and `storage` stay framework-free per the workspace architecture
//! rules.
//!
//! # Scope
//! - [`HttpService`] — [`runtime_core::Service`] implementation that
//!   serves `/eth/v1/...` head endpoints, backed by an
//!   `Arc<dyn storage::Store>` injected at construction.
//! - [`HttpError`] — public error surface returned to clients.
//!
//! # Architecture
//! The crate intentionally depends only on `runtime-core` (for the
//! `Service` trait and shared `httpsvc::Server`), `storage` (for the
//! `Store` trait + `HeadInfo`), and `protocol` (for `Checkpoint`).
//! There are no compile-time references to `engine`, `forkchoice`,
//! `statetransition`, `runtime/chain`, or `runtime/p2p` — composition
//! happens at the `node` crate (Issue #37).

#![forbid(unsafe_code)]

pub mod http;

pub use http::{HttpError, HttpService};
