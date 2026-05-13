//! Adapter that lets the duties [`Service`](crate::Service) drive
//! [`runtime_chain::Service`] through the [`Chain`](crate::Chain)
//! port.
//!
//! Lives in this crate (rather than `runtime-chain`) because the
//! orphan rule requires `impl Trait for Type` to be defined alongside
//! either the trait or the type. The trait `Chain` is owned by
//! `runtime-duties`; the type [`runtime_chain::Service`] is owned by
//! `runtime-chain`. Putting the impl here keeps `runtime-chain` free
//! of any duties dependency.

use async_trait::async_trait;
use protocol::{SignedBlock, SignedVote, Slot, ValidatorIndex};
use runtime_chain::{ChainError, Service as ChainService};

use crate::ports::Chain;

#[async_trait]
impl Chain for ChainService {
    async fn produce_block(
        &self,
        slot: Slot,
        validator: ValidatorIndex,
    ) -> Result<SignedBlock, ChainError> {
        ChainService::produce_block(self, slot, validator).await
    }

    async fn produce_attestation(
        &self,
        slot: Slot,
        validator: ValidatorIndex,
    ) -> Result<SignedVote, ChainError> {
        ChainService::produce_attestation(self, slot, validator).await
    }
}
