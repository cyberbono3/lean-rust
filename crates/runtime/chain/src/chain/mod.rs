//! Chain service module.
//!
//! - [`Service`] — the single engine writer; implements
//!   [`runtime_core::Service`].
//! - [`ChainSnapshot`] — hot-read snapshot consumed by non-writer
//!   services through a shared `Arc<RwLock<_>>`.
//! - [`ChainError`] — infrastructure-level failures from the chain
//!   service (storage, engine invariant violations, tick).

mod cache;
mod error;
mod service;
mod tick;

pub use cache::ChainSnapshot;
pub use error::ChainError;
pub use service::Service;
