//! [`DevnetHost::build`] — the front door for constructing a host.
//!
//! Wires identity, bootnodes, transport, and behaviour into a
//! [`crate::service::P2pService`] without starting the swarm-poll task.
//! `Service::start` (in [`crate::service`]) binds the listener and
//! brings the swarm up.

use libp2p::{swarm::Config as SwarmConfig, Swarm};
use tracing::{debug, info};

use crate::error::HostResult;
use crate::host::{behaviour::DevnetBehaviour, bootnodes, identity, transport};
use crate::options::HostOptions;
use crate::service::P2pService;

/// Front-door builder. Construction-only — does not bind the listener.
pub struct DevnetHost;

impl DevnetHost {
    /// Builds a [`P2pService`] from the supplied options.
    ///
    /// The returned service is in the `Idle` lifecycle state; call
    /// `Service::start` to bind the listener and spawn the swarm-poll
    /// task.
    ///
    /// # Errors
    /// Forwards any failure from identity load/generate, bootnode
    /// parse, or behaviour construction.
    pub fn build(options: HostOptions) -> HostResult<P2pService> {
        let keypair = identity::load_or_generate(options.identity_path())?;
        let peer_id = keypair.public().to_peer_id();

        let bootnodes = options
            .bootnodes_path()
            .map(bootnodes::load)
            .transpose()?
            .unwrap_or_default();
        debug!(count = bootnodes.len(), "loaded bootnodes");

        let behaviour = DevnetBehaviour::build(&keypair, options.agent_version())?;
        let transport = transport::build(&keypair);

        // The tokio executor wires swarm-poll tasks onto the ambient
        // tokio runtime.
        let swarm = Swarm::new(
            transport,
            behaviour,
            peer_id,
            SwarmConfig::with_tokio_executor(),
        );

        info!(%peer_id, "constructed libp2p host");
        Ok(P2pService::new(options, peer_id, swarm, bootnodes))
    }
}
