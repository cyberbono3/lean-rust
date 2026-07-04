//! Publish path: typed [`Host`] methods that dispatch
//! [`HostCommand::Publish`] to the swarm-poll task and await the result.
//!
//! Each publish builds the wire payload via [`lean_wire::encode_gossip`]
//! (SSZ + Snappy block compression), hands it to the swarm task with the
//! topic + a oneshot reply channel, and surfaces the libp2p
//! [`gossipsub::PublishError`] as a typed [`PublishError`] for callers.

use libp2p::gossipsub;
use protocol::{SignedBlock, SignedVote};
use tokio::sync::oneshot;

use crate::p2p::host::{Host, HostCommand};

use super::Topic;

/// Re-export of the libp2p gossipsub message-id type so callers do not
/// need to depend on `libp2p` directly.
pub use libp2p::gossipsub::MessageId;

/// Failure surface for [`Host::publish_block`] and
/// [`Host::publish_vote`].
#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    /// The host command channel is closed — the swarm-poll task has
    /// already exited (typically `Service::stop` ran).
    #[error("host command channel closed")]
    ChannelClosed,
    /// libp2p gossipsub rejected the publish. The most common variant
    /// is [`gossipsub::PublishError::InsufficientPeers`] when no mesh
    /// peer is currently subscribed to the topic.
    #[error("gossipsub publish: {0}")]
    Gossipsub(#[from] gossipsub::PublishError),
}

impl Host {
    /// SSZ-encodes + snappy-compresses `block`, then publishes it on
    /// the [`Topic::Block`] gossipsub topic.
    ///
    /// # Errors
    /// - [`PublishError::ChannelClosed`] if the swarm-poll task has
    ///   exited.
    /// - [`PublishError::Gossipsub`] for any libp2p-surfaced publish
    ///   failure (most often `InsufficientPeers` until the mesh forms).
    pub async fn publish_block(&self, block: &SignedBlock) -> Result<MessageId, PublishError> {
        self.publish_raw(Topic::Block, lean_wire::encode_gossip(block))
            .await
    }

    /// SSZ-encodes + snappy-compresses `vote`, then publishes it on the
    /// [`Topic::Vote`] gossipsub topic.
    ///
    /// # Errors
    /// Same shape as [`Self::publish_block`].
    pub async fn publish_vote(&self, vote: &SignedVote) -> Result<MessageId, PublishError> {
        self.publish_raw(Topic::Vote, lean_wire::encode_gossip(vote))
            .await
    }

    /// Sends a pre-encoded payload to the swarm task and awaits the
    /// gossipsub `publish` result via a oneshot reply channel.
    async fn publish_raw(&self, topic: Topic, payload: Vec<u8>) -> Result<MessageId, PublishError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.commands()
            .send(HostCommand::Publish {
                topic: topic.ident(),
                payload,
                reply: reply_tx,
            })
            .await
            .map_err(|_| PublishError::ChannelClosed)?;
        reply_rx
            .await
            .map_err(|_| PublishError::ChannelClosed)?
            .map_err(PublishError::from)
    }
}
