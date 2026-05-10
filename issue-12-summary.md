# Issue #12 — `protocol` crate: `State` container

Adds the consensus `State` container plus its inner `ProtocolConfig` to the `protocol` crate, with manual SSZ codec and Merkle hash-tree-root.

## Changes

- **`crates/protocol/src/state.rs`** (new) — `ProtocolConfig` (fixed 16 bytes: `num_validators`, `genesis_time`) and `State` (variable-length, 232-byte fixed portion + 4 variable tails). Manual `Encode` / `Decode` / `HashTreeRoot` impls.
- **`crates/protocol/src/internal.rs`** — added crate-private helpers: `BYTES32_LEN`, `encode_bytes32_list`, `decode_bytes32_list`, `bytes32_list_hash_tree_root`, `bitlist_hash_tree_root`.
- **`crates/protocol/src/lib.rs`** — wired `state` module; re-exports `State`, `ProtocolConfig`, `STATE_FIXED_PART_LEN`, `HISTORICAL_ROOTS_LIMIT`, `VALIDATOR_REGISTRY_LIMIT`, `JUSTIFICATIONS_VALIDATORS_LIMIT`.
- **`crates/protocol/tests/state_field_parity.rs`** (new) — declarative field table with 6 parity assertions.

## Wire shape

9 SSZ fields in declaration order:

| # | Field | Shape | Width |
|---|---|---|---|
| 1 | `config` | `ProtocolConfig` (fixed) | 16 |
| 2 | `slot` | `Slot` (fixed) | 8 |
| 3 | `latest_block_header` | `BlockHeader` (fixed) | 112 |
| 4 | `latest_justified` | `Checkpoint` (fixed) | 40 |
| 5 | `latest_finalized` | `Checkpoint` (fixed) | 40 |
| 6 | `historical_block_hashes` | `List[Bytes32, HISTORICAL_ROOTS_LIMIT]` | offset (4) |
| 7 | `justified_slots` | `Bitlist[HISTORICAL_ROOTS_LIMIT]` | offset (4) |
| 8 | `justifications_roots` | `List[Bytes32, HISTORICAL_ROOTS_LIMIT]` | offset (4) |
| 9 | `justifications_validators` | `Bitlist[JUSTIFICATIONS_VALIDATORS_LIMIT]` | offset (4) |

`STATE_FIXED_PART_LEN = 232` bytes (216 fixed-field bytes + 4 × 4-byte offsets). Hash-tree-root merkleizes 9 field roots at width 16 (next power of two).

## Limits (pinned to `config::DEVNET_CONFIG`)

- `HISTORICAL_ROOTS_LIMIT = 262_144`
- `VALIDATOR_REGISTRY_LIMIT = 4_096`
- `JUSTIFICATIONS_VALIDATORS_LIMIT = 262_144 × 4_096 = 1_073_741_824`

## Acceptance criteria

- [x] `HashTreeRoot` impl present (per-field response + determinism asserted; merkleized as `merkleize` of 9 field roots, lists/bitlists via `merkleize_with_limit` + `mix_in_length`).
- [x] Field types, ordering, and sizes recorded in `crates/protocol/tests/state_field_parity.rs` (6 assertions: 9-field count + names in order, 4 variable fields, fixed-portion sum = 232, fixed-field width sum = 216, merkleization width = 16, limits = devnet caps).
- [x] `cargo clippy -p protocol -- -D warnings` clean.
- [ ] Wire-parity test against upstream binary fixtures — fixtures not present in the reference repo; deferred.

## Verification

```bash
cargo fmt --check
cargo clippy -p protocol --all-targets -- -D warnings
cargo test -p protocol state::
```

Result: 16 lib tests + 6 integration parity tests + 7 doctests all pass.
