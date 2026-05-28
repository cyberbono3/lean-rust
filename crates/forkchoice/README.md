# forkchoice

LMD-GHOST fork choice + 4-phase interval ticking (Tier 3).

Tier 3: depends on `protocol` (which owns the `stf`), `config`, `ssz`, and
`types`. No `tokio`, `tracing`, `libp2p`, `runtime`, `networking`, or
`storage` imports — pure, synchronous fork-choice logic.

## Scope

- [`Store`] — data container for blocks, post-states, and validator votes;
  carries the head / safe-target / justified / finalized checkpoints and
  the forkchoice clock. Post-states are held as `Arc<State>` so capture is
  a refcount bump.
- [`Store::from_anchor`](./src/store.rs) — seeds the store from a trusted
  `(state, anchor_block)` pair.
- [`Store::tick_interval`](./src/store.rs) — advances the clock one interval
  and dispatches the spec phase hook.
- [`ProducedBlock`] / [`ProducedVote`] — block / attestation production
  outputs.
- [`Phase`] / [`Time`] — the 4-phase interval clock.
- [`ForkchoiceError`] — crate error type.

A criterion bench (`benches/vote_pool.rs`) records the in-memory vote-pool
entry footprint.

[`Store`]: ./src/store.rs
[`ProducedBlock`]: ./src/production.rs
[`ProducedVote`]: ./src/production.rs
[`Phase`]: ./src/time.rs
[`Time`]: ./src/time.rs
[`ForkchoiceError`]: ./src/error.rs

## Tier and dependencies

Tier 3. Depends on `protocol`, `config`, `ssz`, `types`. No async / I/O —
the runtime `lean-chain` crate wraps this `Store` as the single engine
writer.
