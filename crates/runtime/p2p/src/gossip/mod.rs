//! Gossipsub topic registration, publish path, and inbound routing.
//!
//! Public surface:
//! - [`Topic`] — typed wrapper over the [`networking`] topic constants.
//! - [`MessageId`] / [`PublishError`] — re-exports from [`publisher`].
//! - [`BlockReceiver`] / [`VoteReceiver`] — one-shot ingestion handles
//!   exposed by [`crate::P2pService::take_block_receiver`] /
//!   [`crate::P2pService::take_vote_receiver`].
//!
//! Publish uses an `mpsc::Sender<HostCommand>` + `oneshot` reply so the
//! single-task ownership invariant from the host construction work is
//! preserved. Inbound `gossipsub::Event::Message` is decoded inside the
//! swarm-poll task and forwarded over per-topic `mpsc::Sender`s.

pub(crate) mod handler;
pub(crate) mod publisher;

use libp2p::gossipsub;

pub use handler::{BlockReceiver, GossipReceiver, VoteReceiver};
pub use publisher::{MessageId, PublishError};

/// Typed identifier for the gossipsub topics this crate registers.
///
/// `as_str()` returns the canonical wire string from the [`networking`]
/// crate; `ident()` constructs the libp2p [`gossipsub::IdentTopic`] used
/// at subscribe / publish call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Topic {
    /// Ream-compatible local-pq block topic.
    Block,
    /// Ream-compatible local-pq vote topic.
    Vote,
}

impl Topic {
    /// Canonical wire string for this topic.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Block => networking::BLOCK_TOPIC_V1,
            Self::Vote => networking::VOTE_TOPIC_V1,
        }
    }

    /// libp2p [`gossipsub::IdentTopic`] for this topic. `IdentTopic`
    /// stores the original source string so [`gossipsub::TopicHash::as_str`]
    /// round-trips back to the same value the handler matches against.
    #[must_use]
    pub(crate) fn ident(self) -> gossipsub::IdentTopic {
        gossipsub::IdentTopic::new(self.as_str())
    }

    /// All topics this crate registers — used by `Service::start` to
    /// subscribe in one place.
    pub(crate) const fn all() -> &'static [Topic] {
        &[Topic::Block, Topic::Vote]
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn as_str_matches_networking_constants() {
        assert_eq!(Topic::Block.as_str(), networking::BLOCK_TOPIC_V1);
        assert_eq!(Topic::Vote.as_str(), networking::VOTE_TOPIC_V1);
    }

    #[test]
    fn ident_round_trips_through_topic_hash() {
        for topic in Topic::all() {
            let ident = topic.ident();
            assert_eq!(ident.hash().as_str(), topic.as_str());
        }
    }

    #[test]
    fn all_covers_every_variant() {
        // Exhaustive match guards against forgetting to update `all()`
        // when a new variant is added.
        for topic in Topic::all() {
            match topic {
                Topic::Block | Topic::Vote => {}
            }
        }
        assert_eq!(Topic::all().len(), 2);
    }
}
