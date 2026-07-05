//! [`DevnetHost::build`] / [`DevnetHost::build_with_provider`] — the
//! front doors for constructing a host.
//!
//! Wires identity, bootnodes, transport, and behaviour into a
//! [`crate::p2p::service::P2pService`] without starting the swarm-poll task.
//! `Service::start` (in [`crate::p2p::service`]) binds the listener and
//! brings the swarm up.

use std::sync::Arc;

use libp2p::{swarm::Config as SwarmConfig, Swarm};
use tracing::{debug, info};

use crate::p2p::error::HostResult;
use crate::p2p::host::{behaviour::DevnetBehaviour, bootnodes, keypair, transport};
use crate::p2p::options::HostOptions;
use crate::p2p::rpc::RpcProvider;
use crate::p2p::service::P2pService;

/// Front-door builder. Construction-only — does not bind the listener.
pub struct DevnetHost;

impl DevnetHost {
    /// Builds a [`P2pService`] with a [`RpcProvider::NoOp`] — convenient
    /// for lifecycle tests that do not need real RPC. Operational
    /// deployments must use [`Self::build_with_provider`].
    ///
    /// # Errors
    /// Same shape as [`Self::build_with_provider`].
    pub fn build(options: HostOptions) -> HostResult<P2pService> {
        Self::build_with_provider(options, Arc::new(RpcProvider::NoOp))
    }

    /// Builds a [`P2pService`] backed by the supplied [`RpcProvider`].
    ///
    /// The returned service is in the `Idle` lifecycle state; call
    /// `Service::start` to bind the listener and spawn the swarm-poll
    /// task.
    ///
    /// # Errors
    /// - [`crate::p2p::HostError::IdentityIo`],
    ///   [`crate::p2p::HostError::InvalidIdentity`], or raw-identity
    ///   validation variants on identity-file failures.
    /// - [`crate::p2p::HostError::BootnodesRead`] /
    ///   [`crate::p2p::HostError::BootnodesParse`] /
    ///   [`crate::p2p::HostError::InvalidBootnode`] on bootnode load
    ///   failures.
    /// - [`crate::p2p::HostError::GossipsubInit`] when the composite
    ///   behaviour rejects the internal gossipsub config (programming
    ///   error — config is wholly internal).
    pub fn build_with_provider(
        options: HostOptions,
        provider: Arc<RpcProvider>,
    ) -> HostResult<P2pService> {
        let identity_path = options.identity_path().as_path().to_path_buf();
        let keypair = keypair::load_or_generate(options.identity_path())?;
        let peer_id = keypair.public().to_peer_id();

        let bootnodes = options
            .bootnodes_path()
            .map(bootnodes::load)
            .transpose()?
            .unwrap_or_default();
        let bootnodes_path = options
            .bootnodes_path()
            .map(|path| path.as_path().display().to_string());
        info!(
            path = ?bootnodes_path,
            count = bootnodes.len(),
            "loaded bootnodes",
        );
        for bootnode in &bootnodes {
            debug!(
                peer = %bootnode.peer_id,
                addr = %bootnode.addr,
                "loaded bootnode",
            );
        }

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

        info!(
            peer_id = %peer_id,
            identity_path = %identity_path.display(),
            listen_addr = %options.listen_addr(),
            agent_version = %options.agent_version(),
            "constructed libp2p host",
        );
        Ok(P2pService::new(
            options, peer_id, swarm, bootnodes, provider,
        ))
    }
}
