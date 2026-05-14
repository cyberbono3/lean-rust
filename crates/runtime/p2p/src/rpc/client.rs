//! Outbound RPC client surface on [`Host`].
//!
//! Each method builds an [`RpcRequest`], dispatches a
//! [`HostCommand::SendRequest`] to the swarm-poll task with a oneshot
//! reply channel, and awaits the typed [`RpcResponse`]. The swarm task
//! parks the oneshot in its [`crate::rpc::outbound::OutboundTable`]
//! until the matching libp2p response or failure event fires.

use libp2p::PeerId;
use networking::{BlocksByRootRequest, BlocksByRootResponse};
use tokio::sync::oneshot;

use super::{RpcError, RpcRequest, RpcResponse};
use crate::host::{Host, HostCommand};

impl Host {
    /// Sends a `BlocksByRoot` request to `peer` and awaits the typed
    /// response.
    ///
    /// # Errors
    /// - [`RpcError::ChannelClosed`] if the swarm-poll task has exited
    ///   (typically `Service::stop` ran).
    /// - [`RpcError::Outbound`] for any libp2p-surfaced outbound failure
    ///   (timeout, connection closed, peer ungracefully terminated the
    ///   substream).
    /// - [`RpcError::UnexpectedResponseKind`] if the peer's codec
    ///   returns a non-`BlocksByRoot` response for a `BlocksByRoot`
    ///   request (peer-side programming error).
    pub async fn send_blocks_by_root(
        &self,
        peer: PeerId,
        request: BlocksByRootRequest,
    ) -> Result<BlocksByRootResponse, RpcError> {
        let response = self
            .send_request(peer, RpcRequest::BlocksByRoot(request))
            .await?;
        match response {
            RpcResponse::BlocksByRoot(r) => Ok(r),
            RpcResponse::Status(_) => Err(RpcError::UnexpectedResponseKind {
                expected: "blocks_by_root",
            }),
        }
    }

    /// Common dispatch: builds the command, sends, awaits the oneshot.
    async fn send_request(
        &self,
        peer: PeerId,
        request: RpcRequest,
    ) -> Result<RpcResponse, RpcError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.commands()
            .send(HostCommand::SendRequest {
                peer,
                request,
                reply: reply_tx,
            })
            .await
            .map_err(|_| RpcError::ChannelClosed)?;
        reply_rx.await.map_err(|_| RpcError::ChannelClosed)?
    }
}
