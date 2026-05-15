//! libp2p QUIC-v1 host construction for the runtime shell.
//!
//! # Scope
//! - [`DevnetHost::build`] / [`DevnetHost::build_with_provider`] —
//!   assemble identity, transport, behaviour, and bootnodes into a
//!   [`P2pService`] without starting it.
//! - [`P2pService`] — [`runtime_core::Service`] implementation that
//!   binds the listener at `start`, drives the swarm under a tokio
//!   task, and drains on `stop`.
//! - [`Host`] — clone-friendly handle the rest of the node interacts
//!   with the swarm through.
//! - [`gossip`] — gossipsub topic registration, publish
//!   ([`Host::publish_block`] / [`Host::publish_vote`]), and inbound
//!   routing ([`P2pService::take_block_receiver`] /
//!   [`P2pService::take_vote_receiver`]).
//! - [`rpc`] — `Status` handshake (fire-and-validate on every
//!   `ConnectionEstablished`; mismatched peers are disconnected) and
//!   `BlocksByRoot` lookup. Each protocol owns its own
//!   `request_response::Behaviour` so multistream-select negotiates
//!   the correct wire protocol per request. Inbound handlers source
//!   the local `Status` and answer block-by-root queries through the
//!   pluggable [`RpcProvider`] passed at construction; the default
//!   [`NoOpRpcProvider`] keeps the lifecycle tests free of a storage
//!   backend.

#![forbid(unsafe_code)]

mod devnet;
mod error;
pub mod gossip;
mod host;
mod options;
pub mod rpc;
mod service;
mod wiring;

pub use devnet::DevnetHost;
pub use error::{HostError, HostResult};
pub use gossip::{BlockReceiver, GossipReceiver, MessageId, PublishError, Topic, VoteReceiver};
pub use host::Host;
pub use options::{AgentVersion, BootnodesPath, HostOptions, IdentityPath, ListenAddr};
pub use rpc::{NoOpRpcProvider, RpcError, RpcProvider, RpcRequest, RpcResponse};
pub use service::P2pService;
