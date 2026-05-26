//! Adapter from duties publishing to the running p2p host.

use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use lean_duties::{PublishError, Publisher};
use protocol::{SignedBlock, SignedVote};
use runtime_p2p::P2pService;

/// Forwards validator-duty publish requests to [`runtime_p2p`].
#[derive(Clone)]
pub struct PublisherAdapter {
    p2p: Arc<P2pService>,
}

impl PublisherAdapter {
    /// Builds a publisher adapter around the concrete p2p service.
    #[must_use]
    pub fn new(p2p: Arc<P2pService>) -> Self {
        Self { p2p }
    }

    fn host(&self) -> Result<runtime_p2p::Host, PublishError> {
        self.p2p
            .host()
            .ok_or_else(|| anyhow!("p2p host is not running").into())
    }
}

#[async_trait]
impl Publisher for PublisherAdapter {
    async fn publish_block(&self, block: SignedBlock) -> Result<(), PublishError> {
        self.host()?
            .publish_block(&block)
            .await
            .map(|_| ())
            .map_err(|err| anyhow!("p2p publish block failed: {err}").into())
    }

    async fn publish_attestation(&self, vote: SignedVote) -> Result<(), PublishError> {
        self.host()?
            .publish_vote(&vote)
            .await
            .map(|_| ())
            .map_err(|err| anyhow!("p2p publish attestation failed: {err}").into())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use lean_core::Service;
    use p2p_rpc::NoOpRpcProvider;
    use runtime_p2p::{DevnetHost, HostOptions};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    struct TestP2p {
        _dir: TempDir,
        service: Arc<P2pService>,
    }

    fn build_p2p() -> TestP2p {
        let dir = tempfile::tempdir().unwrap();
        let options = HostOptions::try_new(
            "/ip4/127.0.0.1/udp/0/quic-v1",
            "test/0.1.0",
            &dir.path().join("identity.pb"),
            None,
        )
        .unwrap();
        let service =
            Arc::new(DevnetHost::build_with_provider(options, Arc::new(NoOpRpcProvider)).unwrap());
        TestP2p { _dir: dir, service }
    }

    #[tokio::test]
    async fn publish_before_p2p_start_reports_missing_host() {
        let p2p = build_p2p();
        let adapter = PublisherAdapter::new(p2p.service);

        let err = adapter
            .publish_block(SignedBlock::default())
            .await
            .unwrap_err();

        assert!(
            err.to_string().contains("p2p host is not running"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn publish_after_p2p_start_reaches_host() {
        let p2p = build_p2p();
        p2p.service.start().await.unwrap();
        let adapter = PublisherAdapter::new(Arc::clone(&p2p.service));

        let err = adapter
            .publish_block(SignedBlock::default())
            .await
            .expect_err("publish without mesh peers should surface p2p publish error");

        assert!(
            err.to_string().contains("gossipsub publish"),
            "expected p2p publish error, got {err}"
        );

        p2p.service.stop(CancellationToken::new()).await.unwrap();
    }
}
