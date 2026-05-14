//! libp2p QUIC-v1 host construction for the runtime shell.
//!
//! # Scope
//! - [`DevnetHost::build`] — assemble identity, transport, behaviour,
//!   and bootnodes into a [`P2pService`] without starting it.
//! - [`P2pService`] — [`runtime_core::Service`] implementation that
//!   binds the listener at `start`, drives the swarm under a tokio
//!   task, and drains on `stop`.
//! - [`Host`] — clone-friendly handle the rest of the node interacts
//!   with the swarm through.
//! - [`gossip`] — gossipsub topic registration, publish (`Host::publish_block`
//!   / `Host::publish_vote`), and inbound routing
//!   ([`P2pService::take_block_receiver`] / [`P2pService::take_vote_receiver`]).
//!
//! Request/response handler logic ships as a stub so the wire surface is
//! reserved without forcing a half-finished implementation; the actual
//! `Status` / `BlocksByRoot` handlers land in later milestones.

#![forbid(unsafe_code)]

mod devnet;
mod error;
pub mod gossip;
mod host;
mod local;
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
