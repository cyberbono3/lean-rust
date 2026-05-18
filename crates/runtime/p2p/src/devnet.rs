//! [`DevnetHost::build`] / [`DevnetHost::build_with_provider`] — the
//! front doors for constructing a host.
//!
//! Wires identity, bootnodes, transport, and behaviour into a
//! [`crate::service::P2pService`] without starting the swarm-poll task.
//! `Service::start` (in [`crate::service`]) binds the listener and
//! brings the swarm up.

use std::sync::Arc;

use libp2p::{swarm::Config as SwarmConfig, Swarm};
use tracing::{debug, info};

use crate::error::HostResult;
use crate::host::{behaviour::DevnetBehaviour, bootnodes, keypair, transport};
use crate::options::HostOptions;
use crate::rpc::{NoOpRpcProvider, RpcProvider};
use crate::service::P2pService;

/// Front-door builder. Construction-only — does not bind the listener.
pub struct DevnetHost;

impl DevnetHost {
    /// Builds a [`P2pService`] with a [`NoOpRpcProvider`] — convenient
    /// for lifecycle tests that do not need real RPC. Operational
    /// deployments must use [`Self::build_with_provider`].
    ///
    /// # Errors
    /// Same shape as [`Self::build_with_provider`].
    pub fn build(options: HostOptions) -> HostResult<P2pService> {
        Self::build_with_provider(options, Arc::new(NoOpRpcProvider))
    }

    /// Builds a [`P2pService`] backed by the supplied [`RpcProvider`].
    ///
    /// The returned service is in the `Idle` lifecycle state; call
    /// `Service::start` to bind the listener and spawn the swarm-poll
    /// task.
    ///
    /// # Errors
    /// - [`crate::HostError::IdentityIo`],
    ///   [`crate::HostError::InvalidIdentity`], or raw-identity
    ///   validation variants on identity-file failures.
    /// - [`crate::HostError::BootnodesRead`] /
    ///   [`crate::HostError::BootnodesParse`] /
    ///   [`crate::HostError::InvalidBootnode`] on bootnode load
    ///   failures.
    /// - [`crate::HostError::GossipsubInit`] when the composite
    ///   behaviour rejects the internal gossipsub config (programming
    ///   error — config is wholly internal).
    pub fn build_with_provider(
        options: HostOptions,
        provider: Arc<dyn RpcProvider>,
    ) -> HostResult<P2pService> {
        let keypair = keypair::load_or_generate(options.identity_path())?;
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
        Ok(P2pService::new(
            options, peer_id, swarm, bootnodes, provider,
        ))
    }
}
