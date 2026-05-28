//! Adapter that lets the duties [`Service`](crate::Service) drive
//! [`lean_chain::Service`] through the [`Chain`](crate::Chain)
//! port.
//!
//! Lives in this crate (rather than `lean-chain`) because the
//! orphan rule requires `impl Trait for Type` to be defined alongside
//! either the trait or the type. The trait `Chain` is owned by
//! `lean-duties`; the type [`lean_chain::Service`] is owned by
//! `lean-chain`. Putting the impl here keeps `lean-chain` free
//! of any duties dependency.

use lean_chain::{ChainError, Service as ChainService};
use protocol::{SignedBlock, SignedVote, Slot, ValidatorIndex};

use crate::ports::Chain;

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
