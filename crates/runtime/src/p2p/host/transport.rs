//! QUIC-v1 transport construction.
//!
//! Builds the `libp2p-quic` transport configured for the tokio runtime
//! and boxed to the type expected by [`libp2p::Swarm`]. QUIC-v1 is the
//! only transport this crate supports — TCP + noise + yamux is
//! intentionally out of scope (the wire protocol pins QUIC-v1).

use libp2p::{
    core::{muxing::StreamMuxerBox, transport::Boxed},
    identity::Keypair,
    quic, PeerId, Transport as _,
};

/// Type alias for the `Swarm`-shaped boxed transport this crate yields.
pub(crate) type BoxedTransport = Boxed<(PeerId, StreamMuxerBox)>;

/// Builds a QUIC-v1 transport from the supplied keypair.
///
/// The transport is wired to the ambient tokio runtime and yields
/// `(PeerId, StreamMuxerBox)` per the `Swarm` shape required by the
/// libp2p builder. QUIC subsumes noise + yamux (encryption + muxing
/// baked into the protocol), so no separate auth / mux layers are
/// composed here.
pub(crate) fn build(keypair: &Keypair) -> BoxedTransport {
    let quic_config = quic::Config::new(keypair);
    quic::tokio::Transport::new(quic_config)
        .map(|(peer_id, conn), _| (peer_id, StreamMuxerBox::new(conn)))
        .boxed()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn build_returns_boxed_transport() {
        let keypair = Keypair::generate_ed25519();
        // The smoke-test value here is purely that construction does
        // not panic and yields a transport of the expected boxed type.
        let _transport: BoxedTransport = build(&keypair);
    }
}
