# storage

Narrow persistence layer for the consensus runtime (Tier 4).

Depends on `protocol` and `types` only — no `forkchoice` or runtime imports.

## Scope

- [`Store`] — object-safe persistence contract: `save_block` /
  `save_state` / `save_head` plus the atomic [`Store::save_accepted`]
  (block + post-state + head in one call, head written last so a
  mid-call failure never strands the head ahead of its payload).
- [`MemoryStore`] — in-memory adapter for tests and devnet0; overrides
  `save_accepted` to commit all three writes under a single lock.
- [`HeadInfo`] — `(head, finalized)` checkpoint pair, with
  [`HeadInfo::try_new`] validating `finalized.slot <= head.slot` at the
  deserialization seam.
- [`StorageError`] — concrete error enum.

[`Store`]: ./src/store.rs
[`Store::save_accepted`]: ./src/store.rs
[`MemoryStore`]: ./src/memory.rs
[`HeadInfo`]: ./src/store.rs
[`HeadInfo::try_new`]: ./src/store.rs
[`StorageError`]: ./src/error.rs

## Tier and dependencies

Tier 4. Depends on `protocol` and `types`. The only persistent state the
devnet0 node keeps in memory; a disk-backed adapter would implement the same
[`Store`] trait.
