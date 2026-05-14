//! Outbound RPC request correlation.
//!
//! When the swarm task receives a [`crate::host::HostCommand::SendRequest`]
//! command, it calls `request_response::Behaviour::send_request` and
//! gets back an [`OutboundRequestId`]. The reply `oneshot::Sender` is
//! parked in the [`OutboundTable`] keyed by that id; when the matching
//! `Event::Message { Response }` or `Event::OutboundFailure` arrives,
//! the table is drained and the caller is woken.
//!
//! Lives in the swarm-task's stack frame — single-threaded, no locking.

use std::collections::HashMap;

use libp2p::request_response::OutboundRequestId;
use tokio::sync::oneshot;

use super::{RpcError, RpcResponse};

/// Pending outbound requests waiting on a libp2p response or failure
/// event. Drained on either path; the keyed `oneshot::Sender` is moved
/// out and used exactly once.
#[derive(Debug, Default)]
pub(crate) struct OutboundTable {
    pending: HashMap<OutboundRequestId, oneshot::Sender<Result<RpcResponse, RpcError>>>,
}

impl OutboundTable {
    /// Records a pending outbound request. libp2p guarantees fresh ids
    /// per `send_request` call so collisions are unreachable; the
    /// `debug_assert!` catches a future id-recycling regression in
    /// debug/test builds while keeping the release-build cost identical
    /// to a bare `HashMap::insert`.
    pub(crate) fn insert(
        &mut self,
        id: OutboundRequestId,
        reply: oneshot::Sender<Result<RpcResponse, RpcError>>,
    ) {
        let prior = self.pending.insert(id, reply);
        debug_assert!(
            prior.is_none(),
            "OutboundTable id collision: libp2p reused OutboundRequestId {id:?}",
        );
    }

    /// Wakes the pending caller for `id` with the libp2p response.
    /// Silently drops the result if the caller was cancelled (the
    /// oneshot receiver was dropped).
    pub(crate) fn fulfill(&mut self, id: OutboundRequestId, response: RpcResponse) {
        if let Some(reply) = self.pending.remove(&id) {
            let _ = reply.send(Ok(response));
        }
    }

    /// Wakes the pending caller with an `Outbound` error. Same drop
    /// semantics as [`Self::fulfill`].
    pub(crate) fn fail(&mut self, id: OutboundRequestId, reason: impl Into<String>) {
        if let Some(reply) = self.pending.remove(&id) {
            let _ = reply.send(Err(RpcError::Outbound(reason.into())));
        }
    }
}
