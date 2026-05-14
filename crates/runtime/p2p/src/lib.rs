//! libp2p QUIC-v1 host construction for the runtime shell.
//!
//! # Scope
//! - [`DevnetHost::build`] — assemble identity, transport, behaviour,
//!   and bootnodes into a [`P2pService`] without starting it.
//! - [`P2pService`] — [`runtime_core::Service`] implementation that
//!   binds the listener at `start`, drives the swarm under a tokio
//!   task, and drains on `stop`.
//! - [`Host`] — clone-friendly handle the rest of the node interacts
//!   with the swarm through. Subsequent issues extend the handle with
//!   gossip publish / req/resp send surfaces.
//!
//! Topic subscription, gossip publish, and request/response handlers
//! land in follow-up issues. The handler codec ships as a stub here so
//! the wire surface is reserved without forcing a half-finished
//! implementation.

#![forbid(unsafe_code)]

mod devnet;
mod error;
mod host;
mod local;
mod options;
mod service;
mod wiring;

pub use devnet::DevnetHost;
pub use error::{HostError, HostResult};
pub use host::Host;
pub use options::{AgentVersion, BootnodesPath, HostOptions, IdentityPath, ListenAddr};
pub use service::P2pService;
