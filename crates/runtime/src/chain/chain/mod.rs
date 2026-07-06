//! Chain service module.
//!
//! - [`Service`] — the single engine writer; implements
//!   [`crate::core::Service`].
//! - [`ChainSnapshot`] — by-value projection of engine state captured on
//!   demand via [`Service::snapshot`].
//! - [`ChainError`] — infrastructure-level failures from the chain
//!   service (storage, engine invariant violations, tick).

mod cache;
mod error;
mod service;

pub use cache::ChainSnapshot;
pub use error::ChainError;
pub use service::Service;
