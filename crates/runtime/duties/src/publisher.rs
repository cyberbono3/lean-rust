//! Concrete publish surface for the duties scheduler.
//!
//! Folded in from the former `node::PublisherAdapter` when the
//! `Publisher` port trait was collapsed: the scheduler now holds this
//! concrete [`Publisher`] over the running [`P2pService`] rather than a
//! boxed trait-object seam. Publishing forwards to the gossip host.

use std::sync::Arc;

use anyhow::anyhow;
use lean_p2p_host::P2pService;
use protocol::{SignedBlock, SignedVote};
use thiserror::Error;

/// Failure surface for [`Publisher`] operations.
///
/// Newtype around [`anyhow::Error`]: the scheduler treats every publish
/// failure uniformly (warn-log + fold into the publish-health counter).
/// The `#[from]` impl gives `?`-friendly conversion from `anyhow::Error`.
#[derive(Debug, Error)]
#[error(transparent)]
pub struct PublishError(#[from] anyhow::Error);

/// Concrete outbound publish surface: forwards produced blocks / votes
/// to the running libp2p gossip host.
///
/// Holds `Arc<P2pService>`; publishing resolves the live [`Host`] handle
/// per call (available only while the service is `Running`).
///
/// [`Host`]: lean_p2p_host::Host
#[derive(Clone)]
pub struct Publisher {
    p2p: Arc<P2pService>,
}

impl Publisher {
    /// Builds a publisher over the concrete p2p service.
    #[must_use]
    pub fn new(p2p: Arc<P2pService>) -> Self {
        Self { p2p }
    }

    fn host(&self) -> Result<lean_p2p_host::Host, PublishError> {
        self.p2p
            .host()
            .ok_or_else(|| anyhow!("p2p host is not running").into())
    }

    /// Publishes `block` to all interested peers.
    ///
    /// # Errors
    /// Per-call transport failures surface as [`PublishError`]. The
    /// scheduler warn-logs the failure and continues — a publish error
    /// is not a service-terminal condition.
    pub async fn publish_block(&self, block: SignedBlock) -> Result<(), PublishError> {
        self.host()?
            .publish_block(&block)
            .await
            .map(|_| ())
            .map_err(|err| anyhow!("p2p publish block failed: {err}").into())
    }

    /// Publishes `vote` to all interested peers.
    ///
    /// # Errors
    /// As for [`Self::publish_block`].
    pub async fn publish_attestation(&self, vote: SignedVote) -> Result<(), PublishError> {
        self.host()?
            .publish_vote(&vote)
            .await
            .map(|_| ())
            .map_err(|err| anyhow!("p2p publish attestation failed: {err}").into())
    }
}
