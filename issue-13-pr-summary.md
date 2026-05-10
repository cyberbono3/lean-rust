# Issue #13 — `genesis_state` + slot processing on `State`

Implements Issue #13 (`statetransition`: `genesis_state` + slot processing). Slot-processing logic lives as inherent methods on `protocol::State` rather than as free functions in `statetransition` (architectural decision: "Option A").

## What changed

### `protocol`
- New `state_transition` module: `StateTransitionError` (`TargetSlotNotInFuture`, `SlotOverflow`), private `advance_slot`, `impl State { process_slot, process_slots }`. Both methods take `&mut self`.
- `Slot::ZERO`, `Slot::ONE` const associated values.
- `impl<const N: usize> From<[u8; N]> for ByteVector<N>` (in `types`).
- `impl<const N: usize> HashTreeRoot for ByteVector<N>` (in `ssz`) — was previously a `protocol`-private helper, now generic.

### `statetransition`
- New crate: `genesis_state(num_validators, genesis_time) -> State` builds the slot-0 state with `body_root` committed to the empty `BlockBody`.
- Re-exports `protocol::StateTransitionError` for convenience.
- Wire-parity integration test against the canonical `genesis-4val.state.{ssz,root.hex}` fixture (HTR `75450897…99900`).
- End-to-end integration test exercising `genesis_state(...).process_slots(target)` across the crate boundary.

### Documentation
- `.claude/prompts/lean-rust-github-issues.md` issues #13/#14/#15 updated to reflect the new API surface (methods on `State` in `protocol`, not free functions in `statetransition`).

## Layering decision

`protocol` now owns state-transition logic in addition to data types. The `.claude/rules/architecture.md` rule's strict layering ("domain → traits → adapters → services") is bent to enable inherent-method ergonomics (`state.process_slots(t)` instead of `process_slots(state, t)`). Trade-off accepted explicitly. Future state-transition work (#14, #15) will follow the same pattern.

## API

```rust
use protocol::Slot;
use statetransition::genesis_state;

let mut state = genesis_state(4, 1_700_000_000);
state.process_slots(Slot::new(3))?;
assert_eq!(state.slot, Slot::new(3));
```

## Test plan

- [x] `cargo fmt --check`
- [x] `cargo clippy --all-targets -- -D warnings` clean
- [x] `cargo test -p protocol state_transition::` — 11 tests pass
- [x] `cargo test -p statetransition` — 3 lib + 3 wire-parity + 3 slot-processing integration + 2 doctests pass
- [x] Wire-parity gate: `genesis_state(4, 1_700_000_000)` HTR matches canonical fixture
- [x] Purity gate: `cargo metadata` shows `statetransition` deps = `{config, proptest, protocol, ssz, thiserror, types}` — no `tokio`/`tracing`/`libp2p`/`runtime-*`/`networking`/`storage`

## Acceptance criteria — all met

- [x] `genesis_state(default_config)` parity vs canonical fixture
- [x] `process_slots` errors on `target_slot <= state.slot` (`TargetSlotNotInFuture`)
- [x] Pure: `protocol` and `statetransition` carry no forbidden deps
- [x] Clippy clean
