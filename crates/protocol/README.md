# protocol

Domain consensus types for the Lean Ethereum devnet0 client (Tier 2).

Tier 2: depends on `types` (foundation) and `ssz` (encode / decode /
merkleization) only. No `tracing`, `libp2p`, or runtime imports — the
domain layer stays infrastructure-free.

## Scope

- [`Slot`] — `u64` newtype with SSZ codec and the 3SF-mini
  `is_justifiable_after` rule.
- [`ValidatorIndex`] / [`is_proposer`] — registry-index newtype + the
  round-robin proposer rule (`slot % num_validators`).
- [`Checkpoint`] — `(root, slot)` container with SSZ codec + hash-tree-root.
- [`Vote`] / [`SignedVote`] — unsigned validator vote and its wire-shape
  container (validator id + PQ-signature placeholder).
- [`Block`] / [`BlockBody`] / [`BlockHeader`] / [`SignedBlock`] /
  [`MAX_ATTESTATIONS`] — block containers + attestation cap.
- [`State`] + [`ProtocolConfig`] and the bound constants
  ([`HISTORICAL_ROOTS_LIMIT`], [`VALIDATOR_REGISTRY_LIMIT`],
  [`JUSTIFICATIONS_VALIDATORS_LIMIT`], [`STATE_FIXED_PART_LEN`]) — the
  9-field consensus state container with SSZ codec + HTR.
- [`stf`](./src/stf.rs) — the state-transition function (`process_slot` /
  `process_block`).
- [`error`](./src/error.rs) — [`ProtocolError`], [`StateTransitionError`],
  [`AttSlotKind`].

## Modules of note

- [`ream`](./src/ream.rs) — cross-client genesis interop: the decoder for
  the compact "leanchain" genesis-state SSZ shared with the ream client
  (`State::from_ream_legacy_ssz_bytes`), plus the test pinning the native
  state-root shape to the cross-client nine-field shape. Kept out of
  `state.rs` so the canonical container stays interop-free.

[`Slot`]: ./src/slot.rs
[`ValidatorIndex`]: ./src/validator.rs
[`is_proposer`]: ./src/validator.rs
[`Checkpoint`]: ./src/checkpoint.rs
[`Vote`]: ./src/vote.rs
[`SignedVote`]: ./src/vote.rs
[`Block`]: ./src/block.rs
[`BlockBody`]: ./src/block.rs
[`BlockHeader`]: ./src/block.rs
[`SignedBlock`]: ./src/block.rs
[`MAX_ATTESTATIONS`]: ./src/block.rs
[`State`]: ./src/state.rs
[`ProtocolConfig`]: ./src/state.rs
[`HISTORICAL_ROOTS_LIMIT`]: ./src/state.rs
[`VALIDATOR_REGISTRY_LIMIT`]: ./src/state.rs
[`JUSTIFICATIONS_VALIDATORS_LIMIT`]: ./src/state.rs
[`STATE_FIXED_PART_LEN`]: ./src/state.rs
[`ProtocolError`]: ./src/error.rs
[`StateTransitionError`]: ./src/error.rs
[`AttSlotKind`]: ./src/error.rs

## Tier and dependencies

Tier 2. Depends on `types`, `ssz`, and `config` (for the devnet0 bound
constants). No runtime, networking, or storage imports.
