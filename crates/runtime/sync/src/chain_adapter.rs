//! Adapter that lets the sync [`Loop`](crate::Loop) drive
//! [`lean_chain::Service`] through the [`Chain`](crate::Chain)
//! port.
//!
//! Lives in this crate (rather than `lean-chain`) because the
//! orphan rule requires `impl Trait for Type` to be defined alongside
//! either the trait or the type. The trait `Chain` is owned by
//! `runtime-sync`; the type [`lean_chain::Service`] is owned by
//! `lean-chain`. Putting the impl here keeps `lean-chain` free
//! of any sync dependency.

use async_trait::async_trait;
use engine::BlockImportResult;
use lean_chain::{ChainError, Service as ChainService};
use networking::Status;
use protocol::SignedBlock;
use types::Bytes32;

use crate::ports::Chain;

#[async_trait]
impl Chain for ChainService {
    async fn local_status(&self) -> Result<Status, ChainError> {
        Ok(ChainService::local_status(self))
    }

    async fn has_block(&self, root: Bytes32) -> Result<bool, ChainError> {
        ChainService::has_block(self, &root)
    }

    async fn import_block(&self, signed: SignedBlock) -> Result<BlockImportResult, ChainError> {
        ChainService::import_block(self, signed).await
    }
}
